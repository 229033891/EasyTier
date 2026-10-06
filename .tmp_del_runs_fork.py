import json
import subprocess
import sys
import time

REPO = "229033891/EasyTier"


def gh_json(args):
    r = subprocess.run(["gh"] + args, capture_output=True)
    out = r.stdout.decode("utf-8", errors="replace")
    if r.returncode != 0:
        sys.stderr.write(r.stderr.decode("utf-8", errors="replace"))
        raise SystemExit(r.returncode)
    return json.loads(out)


def gh_ok(args):
    r = subprocess.run(["gh"] + args, capture_output=True)
    if r.returncode != 0:
        sys.stderr.write(r.stderr.decode("utf-8", errors="replace"))
        return False
    return True


runs = gh_json(
    [
        "run",
        "list",
        "-R",
        REPO,
        "--limit",
        "100",
        "--json",
        "databaseId,displayTitle,headBranch,headSha,workflowName,status,conclusion,createdAt,event",
    ]
)
print("listed", len(runs), "from", REPO)

targets = []
for r in runs:
    branch = r.get("headBranch") or ""
    title = r.get("displayTitle") or ""
    sha = r.get("headSha") or ""
    if (
        branch == "releases/v2.7.3e"
        or "2.7.3e" in branch
        or "connection stability" in title.lower()
        or sha.startswith(("61b0d2", "764edb", "ee8d8f", "762e2f", "d8cc1b"))
    ):
        targets.append(r)
        print(
            "HIT",
            r["databaseId"],
            r.get("workflowName"),
            branch,
            sha[:8],
            r.get("status"),
            r.get("conclusion"),
            title[:60],
        )

# Also API pages in case run list misses cancelled/deleted-branch quirks
print("--- api pages ---")
for page in range(1, 5):
    d = gh_json(
        [
            "api",
            f"repos/{REPO}/actions/runs?per_page=100&page={page}",
        ]
    )
    for x in d.get("workflow_runs", []):
        hb = x.get("head_branch") or ""
        sha = x.get("head_sha") or ""
        title = x.get("display_title") or ""
        if (
            hb == "releases/v2.7.3e"
            or "2.7.3e" in hb
            or "connection stability" in title.lower()
            or sha.startswith(("61b0d2cb", "764edb92", "ee8d8f83", "762e2f14", "d8cc1bb3"))
        ):
            if not any(t["databaseId"] == x["id"] for t in targets):
                targets.append(
                    {
                        "databaseId": x["id"],
                        "workflowName": x.get("name"),
                        "headBranch": hb,
                        "headSha": sha,
                        "status": x.get("status"),
                        "conclusion": x.get("conclusion"),
                        "displayTitle": title,
                    }
                )
                print(
                    "API HIT",
                    x["id"],
                    x.get("name"),
                    hb,
                    sha[:8],
                    x.get("status"),
                    title[:50],
                )

ids = sorted({t["databaseId"] for t in targets})
print("TOTAL to delete", len(ids))

# Cancel in-progress first, then delete
deleted = 0
failed = 0
for rid in ids:
    # cancel if running
    gh_ok(["run", "cancel", "-R", REPO, str(rid)])
    # delete
    ok = gh_ok(["api", "-X", "DELETE", f"repos/{REPO}/actions/runs/{rid}"])
    if ok:
        deleted += 1
        print("deleted", rid)
    else:
        failed += 1
        print("FAILED", rid)
    time.sleep(0.2)

print(f"done deleted={deleted} failed={failed}")

# verify
left = []
runs2 = gh_json(
    [
        "run",
        "list",
        "-R",
        REPO,
        "--limit",
        "50",
        "--json",
        "databaseId,displayTitle,headBranch,headSha,workflowName",
    ]
)
for r in runs2:
    branch = r.get("headBranch") or ""
    title = r.get("displayTitle") or ""
    sha = r.get("headSha") or ""
    if branch == "releases/v2.7.3e" or "2.7.3e" in branch or "connection stability" in title.lower():
        left.append(r)
print("remaining matching runs among latest 50:", len(left))
for r in left:
    print(r)
