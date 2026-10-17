use core::cmp::Ordering;
use petgraph::{
    algo::Measure,
    visit::{EdgeRef, IntoEdgeReferences, IntoEdges, VisitMap, Visitable},
};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::{collections::BinaryHeap, hash::Hash};

/// `MinScored<K, T>` holds a score `K` and a scored object `T` in
/// a pair for use with a `BinaryHeap`.
///
/// `MinScored` compares in reverse order by the score, so that we can
/// use `BinaryHeap` as a min-heap to extract the score-value pair with the
/// least score.
///
/// **Note:** `MinScored` implements a total order (`Ord`), so that it is
/// possible to use float types as scores.
#[derive(Copy, Clone, Debug)]
pub struct MinScored<K, T>(pub K, pub T);

impl<K: PartialOrd, T> PartialEq for MinScored<K, T> {
    #[inline]
    fn eq(&self, other: &MinScored<K, T>) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<K: PartialOrd, T> Eq for MinScored<K, T> {}

impl<K: PartialOrd, T> PartialOrd for MinScored<K, T> {
    #[inline]
    fn partial_cmp(&self, other: &MinScored<K, T>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: PartialOrd, T> Ord for MinScored<K, T> {
    #[inline]
    fn cmp(&self, other: &MinScored<K, T>) -> Ordering {
        let a = &self.0;
        let b = &other.0;
        if a == b {
            Ordering::Equal
        } else if a < b {
            Ordering::Greater
        } else if a > b {
            Ordering::Less
        } else if a.ne(a) && b.ne(b) {
            // these are the NaN cases
            Ordering::Equal
        } else if a.ne(a) {
            // Order NaN less, so that it is last in the MinScore order
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

pub type DijkstraResult<K, NodeId> = (HashMap<NodeId, K>, HashMap<NodeId, (NodeId, usize)>);

/// Single-source Dijkstra that also reports, for every reachable node, the
/// **first hop** taken and the number of hops.
///
/// Ties are broken by the lexicographic key `(cost, hops, tie_key(first_hop))`:
///
/// - `cost` is primary, as usual.
/// - On equal cost the path with **fewer hops** wins. That is what turns
///   "same quality → prefer the direct link" into a guarantee instead of a
///   side-effect of the order edges happen to be walked in.
/// - `tie_key` must be a **rebuild-stable identity of the node** (callers pass
///   the peer id). Node indices are reassigned on every topology rebuild, so
///   keying on `G::NodeId` would make equal-cost/equal-hop diamonds resolve
///   differently from one rebuild to the next. It is also invoked several times
///   per relaxation, so it must be pure / idempotent.
///
/// With a total order on the key and monotone path extension, a node is safe to
/// finalize on its first pop, so the visited-skip cannot lock in a worse hop
/// count than a candidate still sitting in the heap.
pub fn dijkstra_with_first_hop<G, F, K, NK, TK>(
    graph: G,
    start: G::NodeId,
    mut edge_cost: F,
    mut tie_key: NK,
) -> DijkstraResult<K, G::NodeId>
where
    G: IntoEdges + Visitable,
    G::NodeId: Eq + Hash + Clone,
    F: FnMut(G::EdgeRef) -> K,
    K: Measure + Copy,
    NK: FnMut(G::NodeId) -> TK,
    TK: Ord + Copy,
{
    let mut visited = graph.visit_map();
    let mut scores = HashMap::new();
    let mut first_hop = HashMap::new();
    let mut visit_next = BinaryHeap::new();
    let zero_score = K::default();
    scores.insert(start, zero_score);
    visit_next.push(MinScored((zero_score, 0usize, tie_key(start)), start));
    first_hop.insert(start, (start, 0));

    while let Some(MinScored((node_score, node_hops, _), node)) = visit_next.pop() {
        if visited.is_visited(&node) {
            continue;
        }
        // `first_hop[node]` was written when the winning candidate was pushed,
        // so it already matches this (best-lexicographic) key.
        let node_first = first_hop[&node].0;
        for edge in graph.edges(node) {
            let next = edge.target();
            if visited.is_visited(&next) {
                continue;
            }
            let next_score = node_score + edge_cost(edge);
            let next_hops = node_hops + 1;
            // 继承前驱的 first_hop，或自己就是第一跳
            let next_first = if node == start { next } else { node_first };
            let candidate = (next_score, next_hops, tie_key(next_first));

            let better = match first_hop
                .get(&next)
                .and_then(|(f, h)| scores.get(&next).map(|s| (*s, *h, tie_key(*f))))
            {
                Some(cur) => candidate < cur,
                None => true,
            };
            if better {
                scores.insert(next, next_score);
                first_hop.insert(next, (next_first, next_hops));
                visit_next.push(MinScored(candidate, next));
            }
        }
        visited.visit(node);
    }

    (scores, first_hop)
}

/// Tight-edge parent lists for one `dijkstra_with_first_hop` result.
///
/// A tight edge `(u, v)` satisfies `dist(u) + w(u, v) == dist(v)`, i.e. it lies
/// on some shortest path from the source. The reverse lists this returns are the
/// input to `equal_cost_first_hops`.
///
/// Split out from `equal_cost_first_hops` so a rebuild pays for it **once**
/// instead of once per destination: building it is O(E), while the per
/// destination walk below is O(size of the destination's ancestor set).
pub type TightEdgeParents<NodeId> = HashMap<NodeId, SmallVec<[NodeId; 2]>>;

pub fn tight_edge_parents<G, K, F>(
    graph: G,
    scores: &HashMap<G::NodeId, K>,
    mut edge_cost: F,
) -> TightEdgeParents<G::NodeId>
where
    G: IntoEdgeReferences,
    G::NodeId: Eq + Hash + Copy,
    F: FnMut(G::EdgeRef) -> K,
    K: Measure + Copy,
{
    let mut parents: TightEdgeParents<G::NodeId> = HashMap::new();
    for edge in graph.edge_references() {
        let (from, to) = (edge.source(), edge.target());
        let (Some(from_cost), Some(to_cost)) = (scores.get(&from), scores.get(&to)) else {
            continue;
        };
        if *from_cost + edge_cost(edge) == *to_cost {
            parents.entry(to).or_default().push(from);
        }
    }
    parents
}

/// Equal-cost **alternate** first hops for every reachable node, in one pass.
///
/// ## Why equal cost, and not "within a margin"
///
/// This is what can be proven loop-free from local data alone. Let `us` be
/// `start` and `X` a candidate first hop. `X` can only send the packet back to
/// us if its own best path to `dst` goes through `us`, i.e.
/// `dist(X, dst) == w(X, us) + dist(us, dst)` (★). Being an equal-cost first hop
/// means `dist(us, dst) == w(us, X) + dist(X, dst)` (☆). Substituting (★) into
/// (☆) gives `w(us, X) + w(X, us) == 0`, which is impossible with positive edge
/// weights. So an equal-cost alternate **cannot** route back through us, using
/// nothing but the local graph.
///
/// A "cheaper-or-within-a-margin" candidate has no such argument: the same
/// substitution leaves a positive residue, and closing the hole needs the
/// neighbour's own distance to `dst` — that is the RFC 8402 LFA computation,
/// which is deliberately **not** attempted here (see the roadmap doc §5.2).
///
/// ## Why one pass, not one walk per destination
///
/// The first-hop set of a node is the union of its own primary first hop and the
/// sets of its tight-edge parents — because any node on an optimal path to `v`
/// contributes its own first hop as an equally cheap first hop for `v`. Tight
/// edges always go from a smaller distance to a larger one, so processing nodes
/// in **decreasing distance order** makes that recurrence a single sweep:
/// `O(V log V)` for the sort plus `O(E × MAX_ALTERNATE_NEXT_HOPS)` for the merges,
/// independent of how many destinations there are.
///
/// (A per-destination backward walk instead costs `O(V)` for *every* destination
/// — that is `O(V²)` overall, and it dominates the rebuild on large meshes even
/// when no destination has an alternate at all.)
///
/// `parents` must come from `tight_edge_parents` for the same graph and `scores`.
pub fn equal_cost_alternates_for_all<NodeId, K>(
    scores: &HashMap<NodeId, K>,
    first_hop: &HashMap<NodeId, (NodeId, usize)>,
    parents: &TightEdgeParents<NodeId>,
    start: NodeId,
    max: usize,
) -> HashMap<NodeId, SmallVec<[NodeId; 2]>>
where
    NodeId: Eq + Hash + Copy + Ord,
    K: Measure + Copy,
{
    // **Increasing** distance: a tight edge always goes from a smaller distance
    // to a larger one, so every parent is resolved before the node that needs it.
    let mut order: Vec<(K, NodeId)> = scores.iter().map(|(node, cost)| (*cost, *node)).collect();
    order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    // Full first-hop sets (primary first), capped at `max + 1` entries.
    let mut full: HashMap<NodeId, SmallVec<[NodeId; 3]>> = HashMap::new();
    for (_, node) in order {
        let Some(primary) = first_hop.get(&node).map(|(hop, _)| *hop) else {
            continue;
        };
        let mut set: SmallVec<[NodeId; 3]> = SmallVec::new();
        set.push(primary);
        if let Some(node_parents) = parents.get(&node) {
            for &parent in node_parents.iter() {
                // `start` itself has no first hop; its children seed the sets.
                if parent == start {
                    continue;
                }
                let Some(parent_set) = full.get(&parent) else {
                    continue;
                };
                for hop in parent_set.iter() {
                    if set.len() > max {
                        break;
                    }
                    if !set.contains(hop) {
                        set.push(*hop);
                    }
                }
            }
        }
        full.insert(node, set);
    }

    // Drop the primary: callers already have it, and it must stay first on the
    // send path.
    full.into_iter()
        .map(|(node, set)| {
            let alts: SmallVec<[NodeId; 2]> = set.iter().skip(1).copied().take(max).collect();
            (node, alts)
        })
        .collect()
}

/// Every first hop that lies on an **equal-cost** shortest path from `start` to
/// `dst`, capped at `max`. Single-destination convenience used by the tests;
/// `equal_cost_alternates_for_all` is the production path (one sweep for all
/// destinations).
#[cfg(test)]
pub fn equal_cost_first_hops<NodeId: Eq + Hash + Copy + Ord, K: Measure + Copy>(
    scores: &HashMap<NodeId, K>,
    first_hop: &HashMap<NodeId, (NodeId, usize)>,
    parents: &TightEdgeParents<NodeId>,
    start: NodeId,
    dst: NodeId,
    max: usize,
) -> SmallVec<[NodeId; 2]> {
    let all = equal_cost_alternates_for_all(scores, first_hop, parents, start, max);
    all.get(&dst).cloned().unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    use petgraph::graph::DiGraph;

    #[test]
    fn test_dijkstra_with_first_hop_4node() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let c = graph.add_node("c");
        let d = graph.add_node("d");

        graph.extend_with_edges([(a, b, 1)]);
        graph.extend_with_edges([(b, c, 1)]);
        graph.extend_with_edges([(c, d, 2)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );

        assert_eq!(scores[&b], 1);
        assert_eq!(scores[&c], 2);
        assert_eq!(scores[&d], 4);

        assert_eq!(first_hop[&b], (b, 1));
        assert_eq!(first_hop[&c], (b, 2));
        assert_eq!(first_hop[&d], (b, 3));
    }

    #[test]
    fn test_dijkstra_with_first_hop() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let c = graph.add_node("c");
        let d = graph.add_node("d");
        let e = graph.add_node("e");

        graph.extend_with_edges([(a, b, 1), (a, c, 2), (b, d, 1), (c, d, 3), (d, e, 1)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );

        assert_eq!(scores[&b], 1);
        assert_eq!(scores[&c], 2);
        assert_eq!(scores[&d], 2);
        assert_eq!(scores[&e], 3);

        assert_eq!(first_hop[&b], (b, 1));
        assert_eq!(first_hop[&c], (c, 1));
        assert_eq!(first_hop[&d], (b, 2)); // d is reached via b
        assert_eq!(first_hop[&e], (b, 3)); // e is reached via d
    }

    /// Equal cost must not let a longer path win just because it was discovered
    /// first. Here `q -> r -> z` (3 hops) reaches `z` at cost 6 before
    /// `p -> z` (2 hops) does, so the plain "first score wins" rule would keep
    /// the 3-hop first hop.
    #[test]
    fn equal_cost_prefers_the_fewer_hop_path() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let p = graph.add_node("p");
        let q = graph.add_node("q");
        let r = graph.add_node("r");
        let z = graph.add_node("z");

        graph.extend_with_edges([(a, p, 3), (p, z, 3), (a, q, 1), (q, r, 1), (r, z, 4)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );

        assert_eq!(scores[&z], 6);
        assert_eq!(
            first_hop[&z],
            (p, 2),
            "2 hops must beat the earlier 3-hop path"
        );
    }

    /// The hop-count preference must stay *secondary*: a strictly cheaper
    /// multi-hop path still wins. This is the guard against "prefer direct" being
    /// over-applied — e.g. a ~13ms direct link vs a 2-hop path costing ~2.
    #[test]
    fn strictly_cheaper_multi_hop_path_still_wins() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let d = graph.add_node("d");

        graph.extend_with_edges([(a, d, 13), (a, b, 1), (b, d, 1)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );

        assert_eq!(scores[&d], 2);
        assert_eq!(
            first_hop[&d],
            (b, 2),
            "cost is primary; hops only break ties"
        );
    }

    /// Equal cost and equal hops is the only true tie left. It must resolve by
    /// the node's stable key (here the node weight), never by insertion order.
    #[test]
    fn equal_cost_equal_hops_is_stable_across_edge_order() {
        let build = |edges: [(usize, usize, u32); 4]| {
            let mut graph = DiGraph::<&str, u32>::new();
            let n = [
                graph.add_node("a"),
                graph.add_node("b"),
                graph.add_node("c"),
                graph.add_node("d"),
            ];
            for (src, dst, w) in edges {
                graph.add_edge(n[src], n[dst], w);
            }
            let (scores, first_hop) = dijkstra_with_first_hop(
                &graph,
                n[0],
                |edge| *edge.weight(),
                |id| *graph.node_weight(id).unwrap(),
            );
            (
                *graph.node_weight(first_hop[&n[3]].0).unwrap(),
                scores[&n[3]],
            )
        };

        // a -> b -> d and a -> c -> d, both cost 2 over 2 hops.
        let forward = build([(0, 1, 1), (1, 3, 1), (0, 2, 1), (2, 3, 1)]);
        let reversed = build([(0, 2, 1), (2, 3, 1), (0, 1, 1), (1, 3, 1)]);

        assert_eq!(forward, reversed);
        assert_eq!(forward, ("b", 2), "the smaller node key wins the tie");
    }

    /// Equal-cost alternates are the only case with a local loop-freedom proof
    /// (see `equal_cost_first_hops`), so the diamond must report both of them.
    #[test]
    fn equal_cost_alternates_are_all_reported() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let c = graph.add_node("c");
        let d = graph.add_node("d");
        graph.extend_with_edges([(a, b, 1), (a, c, 1), (b, d, 1), (c, d, 1)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );
        let parents = tight_edge_parents(&graph, &scores, |e| *e.weight());
        let alts = equal_cost_first_hops(&scores, &first_hop, &parents, a, d, 3);

        // The returned set excludes the primary, so add it back to check that
        // both equal-cost paths are known.
        let primary = first_hop[&d].0;
        let mut names: Vec<&str> = alts
            .iter()
            .chain(std::iter::once(&primary))
            .map(|n| *graph.node_weight(*n).unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["b", "c"]);
        assert_eq!(scores[&d], 2, "both alternates are on cost-2 paths");
    }

    /// A strictly worse path must NOT be offered as an alternate: `b` is the
    /// primary here, `c` is genuinely more expensive.
    #[test]
    fn costlier_paths_are_not_alternates() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let c = graph.add_node("c");
        let d = graph.add_node("d");
        // a -> b -> d costs 2; a -> c -> d costs 6.
        graph.extend_with_edges([(a, b, 1), (b, d, 1), (a, c, 3), (c, d, 3)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );
        let parents = tight_edge_parents(&graph, &scores, |e| *e.weight());
        let alts = equal_cost_first_hops(&scores, &first_hop, &parents, a, d, 3);

        assert_eq!(*graph.node_weight(first_hop[&d].0).unwrap(), "b");
        assert!(alts.is_empty(), "expected no alternate, got {alts:?}");
    }

    /// A direct edge participates in the equal-cost set: here it wins the hop
    /// tie as the primary, and the two-hop detour is the alternate.
    #[test]
    fn direct_edge_wins_the_tie_and_the_detour_is_the_alternate() {
        let mut graph = DiGraph::<&str, u32>::new();
        let a = graph.add_node("a");
        let b = graph.add_node("b");
        let d = graph.add_node("d");
        graph.extend_with_edges([(a, d, 2), (a, b, 1), (b, d, 1)]);

        let (scores, first_hop) = dijkstra_with_first_hop(
            &graph,
            a,
            |edge| *edge.weight(),
            |n| *graph.node_weight(n).unwrap(),
        );
        let parents = tight_edge_parents(&graph, &scores, |e| *e.weight());
        let alts = equal_cost_first_hops(&scores, &first_hop, &parents, a, d, 3);

        assert_eq!(scores[&d], 2);
        assert_eq!(
            *graph.node_weight(first_hop[&d].0).unwrap(),
            "d",
            "the direct edge is 1 hop, so it wins the equal-cost tie"
        );
        let names: Vec<&str> = alts
            .iter()
            .map(|n| *graph.node_weight(*n).unwrap())
            .collect();
        assert_eq!(names, vec!["b"]);
    }

    /// Synthetic symmetric mesh: every node links to the next `k` nodes, both
    /// directions, all edges cost 1.
    fn mesh(n: u32, k: u32) -> DiGraph<u32, usize> {
        let mut g = DiGraph::<u32, usize>::new();
        let nodes: Vec<_> = (0..n).map(|i| g.add_node(i)).collect();
        for i in 0..n {
            for step in 1..=k {
                let j = (i + step) % n;
                if i == j {
                    continue;
                }
                g.add_edge(nodes[i as usize], nodes[j as usize], 1);
                g.add_edge(nodes[j as usize], nodes[i as usize], 1);
            }
        }
        g
    }

    /// Not a correctness test — a **measurement harness** for the route-rebuild
    /// cost, so a go/no-go on the full LFA (roadmap §5.2) is based on numbers
    /// rather than guesses.
    ///
    /// Run explicitly:
    /// `cargo test -p easytier-core --lib -- --ignored --nocapture route_rebuild_cost`
    ///
    /// All weights are 1, which is the **worst case** for the alternates walk:
    /// every equal-length path is a shortest path, so the tight-edge DAG is at
    /// its widest.
    #[test]
    #[ignore = "measurement harness, run explicitly with --ignored --nocapture"]
    fn route_rebuild_cost_baseline() {
        use std::hint::black_box;
        use std::time::Instant;

        const ITERS: u32 = 10;

        println!(
            "\n{:>5} {:>4} {:>6} {:>16} {:>18} {:>9} {:>16}",
            "n", "k", "deg", "dijkstra", "alts(all dst)", "alts/dij", "LFA proj."
        );
        for &(n, k) in &[
            (10u32, 2u32),
            (30, 2),
            (30, 5),
            (100, 5),
            (100, 10),
            (300, 10),
        ] {
            let g = mesh(n, k);
            let start = g.node_indices().next().unwrap();
            let degree = g.edges(start).count();

            let run_dijkstra = || {
                dijkstra_with_first_hop(&g, start, |e| *e.weight(), |x| *g.node_weight(x).unwrap())
            };
            let t0 = Instant::now();
            for _ in 0..ITERS {
                black_box(run_dijkstra());
            }
            let dij = t0.elapsed() / ITERS;

            let (scores, first_hop) = run_dijkstra();
            let parents = tight_edge_parents(&g, &scores, |e| *e.weight());

            let t1 = Instant::now();
            for _ in 0..ITERS {
                black_box(equal_cost_alternates_for_all(
                    &scores, &first_hop, &parents, start, 2,
                ));
            }
            let alts = t1.elapsed() / ITERS;

            println!(
                "{n:>5} {k:>4} {degree:>6} {dij:>16?} {alts:>16?} {:>8.2}x {:>16?}",
                alts.as_secs_f64() / dij.as_secs_f64().max(f64::MIN_POSITIVE),
                dij * (degree as u32 + 1),
            );
        }
        println!();
    }
}
