#!/usr/bin/env bash
# Deletes every Cloudflare Pages preview deployment of a branch, e.g. `pr-42`.
#
# A deployment still being published, e.g. of a PR merged as its preview deploys, can't be deleted
# yet: each pass deletes what it can, and the next tries again what's left, until none is or the
# passes run out. It fails only then, naming the deployments it couldn't delete.
#
#   delete-previews.sh <branch>   deletes the branch's preview deployments
#   delete-previews.sh --branches lists the branches with preview deployments, one a line
#
# Needs CLOUDFLARE_API_TOKEN, ACCOUNT_ID and PROJECT in the environment. PASSES and WAIT, in
# seconds, tune the retries.

set -euo pipefail
# a failure to list within `$(...)` stops the script too, rather than passing for none listed
shopt -s inherit_errexit

branch="${1:?usage: $0 <branch> | --branches}"
passes="${PASSES:-10}"
wait="${WAIT:-30}"
api="https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/pages/projects/$PROJECT/deployments"
auth="Authorization: Bearer $CLOUDFLARE_API_TOKEN"

# each preview deployment, its id and its branch, every page collected first, as deleting while
# paging would shift them
deployments() {
  local page=1 resp
  while :; do
    resp="$(curl -fsS -H "$auth" "$api?env=preview&page=$page&per_page=25")"
    jq -r '.result[] | "\(.id) \(.deployment_trigger.metadata.branch // "")"' <<< "$resp"
    [[ "$(jq '.result | length' <<< "$resp")" -lt 25 ]] && break
    page=$((page + 1))
  done
}

if [[ "$branch" == "--branches" ]]; then
  listing="$(deployments)"
  [[ -n "$listing" ]] && awk 'NF == 2 { print $2 }' <<< "$listing" | sort -u
  exit 0
fi

# the branch's deployments
list() {
  local listing
  listing="$(deployments)"
  [[ -z "$listing" ]] && return 0
  awk -v branch="$branch" '$2 == branch { print $1 }' <<< "$listing"
}

deleted=0
for ((pass = 1; pass <= passes; pass++)); do
  # listed apart, so a failure to list stops the script, rather than passing for none to delete
  listing="$(list)"
  ids=()
  [[ -n "$listing" ]] && mapfile -t ids <<< "$listing"
  left=()
  for id in "${ids[@]}"; do
    # force, so the deployment behind the branch alias goes too
    if out="$(curl -sS -X DELETE -H "$auth" "$api/$id?force=true")" && [[ "$(jq -r '.success' <<< "$out")" == "true" ]]; then
      echo "Deleted $id"
      deleted=$((deleted + 1))
    else
      echo "Couldn't delete $id yet: $(jq -c '.errors // .' <<< "$out" 2> /dev/null || echo "$out")"
      left+=("$id")
    fi
  done
  if [[ "${#left[@]}" -eq 0 ]]; then
    echo "Deleted $deleted deployments of $branch"
    exit 0
  fi
  if ((pass < passes)); then
    echo "Waiting ${wait}s for ${#left[@]} deployments of $branch to finish publishing"
    sleep "$wait"
  fi
done

echo "::error::Couldn't delete ${#left[@]} deployments of $branch: ${left[*]}"
exit 1
