#!/usr/bin/env bash
# 根 Cargo.lock 与 desktop/src-tauri/Cargo.lock 的一致性：共用依赖的版本与根锁一致，Tauri 依赖只补不改。
# 两边都有的包，按兼容版本线（1.x 看主版本，0.y 看次版本，0.0.z 看修订号）比：同一条线上两边的版本必须相同。
# 只在一边出现的版本线不算（Tauri 另带的旧版本、根工作区里外壳用不到的 crate）。
# 不一致就逐行列出「包名: root 版本; desktop 版本」并 exit 1。只用 bash + awk，不依赖 cargo。
# 用法：bash desktop/scripts/check-lock.sh [根 Cargo.lock] [外壳 Cargo.lock]
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root_lock=${1:-$here/../../Cargo.lock}
desktop_lock=${2:-$here/../src-tauri/Cargo.lock}
for lock in "$root_lock" "$desktop_lock"; do
  if [ ! -f "$lock" ]; then
    echo "check-lock: $lock not found" >&2
    exit 2
  fi
done

awk '
  function line_of(v,    parts) {
    sub(/\+.*/, "", v)
    sub(/-.*/, "", v)
    split(v, parts, ".")
    if (parts[1] != "0") return parts[1]
    if (parts[2] != "0") return "0." parts[2]
    return "0.0." parts[3]
  }
  FNR == 1 { file++ }
  /^\[\[package\]\]$/ { name = ""; next }
  /^name = "/ { name = $3; gsub(/"/, "", name); next }
  /^version = "/ && name != "" {
    version = $3; gsub(/"/, "", version)
    if (!((name, version, file) in seen)) {
      seen[name, version, file] = 1
      versions[name, file] = versions[name, file] " " version
      if (file == 1 && !(name in listed)) { order[++count] = name; listed[name] = 1 }
    }
    name = ""
    next
  }
  END {
    shared = 0; bad = 0
    for (i = 1; i <= count; i++) {
      p = order[i]
      if (!((p, 2) in versions)) continue
      shared++
      nr = split(versions[p, 1], root, " ")
      nd = split(versions[p, 2], desk, " ")
      for (r = 1; r <= nr; r++) {
        for (d = 1; d <= nd; d++) {
          if (root[r] != desk[d] && line_of(root[r]) == line_of(desk[d])) {
            printf "%s: root %s; desktop %s\n", p, root[r], desk[d]
            bad++
          }
        }
      }
    }
    if (bad) {
      printf "check-lock: %d version mismatches among %d shared packages\n", bad, shared > "/dev/stderr"
      exit 1
    }
    printf "check-lock: %d shared packages agree\n", shared
  }
' "$root_lock" "$desktop_lock"
