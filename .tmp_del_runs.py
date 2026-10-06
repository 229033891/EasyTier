import json
import subprocess
import sys
import collections


def gh_json(args):
    r = subprocess.run(["gh"] + args, capture_output=True)
    out = r.stdout.decode("utf-8", errors="replace")
    if r.returncode != 0:
        sys.stderr.write(r.stderr.decode("utf-8", errors="replace"))
        raise SystemExit(r.returncode)
    return json.loads(out)


repo = gh_json(["repo", "view", "--json", "nameWithOwner"])
print("repo:", repo.get("nameWithOwner"))

runs = gh_json(
    [
        "run",
        "list",
        "--limit",
        "100",
        "--json",
        "databaseId,displayTitle,headBranch,headSha,workflowName,status,conclusion,createdAt,event",
    ]
)
print("listed", len(runs))
for r in runs[:50]:
    print(
        r["databaseId"],
        r.get("createdAt", ""),
        (r.get("headBranch") or "-"),
        (r.get("workflowName") or "")[:24],
        (r.get("headSha") or "")[:8],
        (r.get("displayTitle") or "")[:55],
    )

print("--- branch counts ---")
print(collections.Counter((r.get("headBranch") or "") for r in runs).most_common(20))

targets = []
for r in runs:
    branch = r.get("headBranch") or ""
    title = r.get("displayTitle") or ""
    sha = r.get("headSha") or ""
    if (
        branch == "releases/v2.7.3e"
        or "2.7.3e" in branch
        or "connection stability" in title.lower()
        or sha.startswith(
            (
                "61b0d2",
                "764edb",
                "ee8d8f",
                "762e2f",
                "d8cc1b",
            )
        )
    ):
        targets.append(r)

print("--- targets from run list ---", len(targets))
for r in targets:
    print(r)

# API scan pages
print("--- api scan pages ---")
for page in range(1, 6):
    d = gh_json(["api", f"repos/:owner/:repo/actions/runs?per_page=100&page={page}"])
    for x in d.get("workflow_runs", []):
        hb = x.get("head_branch") or ""
        sha = x.get("head_sha") or ""
        title = x.get("display_title") or ""
        if (
            hb == "releases/v2.7.3e"
            or "2.7.3e" in hb
            or "connection stability" in title.lower()
            or sha.startswith(("61b0d2cb", "764edb92", "ee8d8f83"))
        ):
            if not any(t.get("databaseId") == x["id"] or t.get("id") == x["id"] for t in targets):
                targets.append(
                    {
                        "databaseId": x["id"],
                        "workflowName": x.get("name"),
                        "headBranch": hb,
                        "headSha": sha,
                        "status": x.get("status"),
                        "conclusion": x.get("conclusion"),
                        "displayTitle": title,
                        "createdAt": x.get("created_at"),
                    }
                )
                print(
                    "API",
                    x["id"],
                    x.get("name"),
                    hb,
                    sha[:8],
                    x.get("status"),
                    x.get("conclusion"),
                    title[:50],
                )

ids = sorted({t.get("databaseId") or t.get("id") for t in targets})
print("TOTAL unique targets", len(ids))
path = r"C:\Users\ADMINI~1\AppData\Local\Temp\2\ids-273e.txt"
with open(path, "w", encoding="utf-8") as f:
    f.write("\n".join(map(str, ids)))
print("wrote", path)
