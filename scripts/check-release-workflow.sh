#!/bin/bash
# Release Desktop 工作流 macOS runner 约束门禁（desktop#156）。
#
# 背景：macos-latest 落到 arm64 镜像后，原工作流把 aarch64/x86_64 拆成两个
# job ⇒ 一趟发布要同时抢到两个 arm64 runner；容量紧张时必有一条腿排队超时
# 被取消（5.9.0 run 43915691142 实录），publish-to-hub 随之跳过。
#
# 修复口径：macOS 两个目标合进**同一个** macos-latest job 顺序构建。
# 本门禁保证：
#   1) matrix 里 macos-latest 只有一条（= 运行时一个 macOS job）；
#   2) 同一 job 内两条 tauri-action 分别构建 aarch64 / x86_64；
#   3) 非 macOS 腿仍走 matrix.args；
#   4) 回挂作业与 workflow_dispatch 重跑能力仍在。
set -euo pipefail

cd "$(dirname "$0")/.."
WF=".github/workflows/release.yml"
FAIL=0

chk() {
    local desc="$1"; shift
    if "$@"; then
        printf '  ✓ %s\n' "$desc"
    else
        printf '  ✗ %s\n' "$desc" >&2
        FAIL=1
    fi
}

has()      { grep -Fq -- "$1" "$WF"; }
count_eq() { [ "$(grep -Fc -- "$1" "$WF")" -eq "$2" ]; }
count_ge() { [ "$(grep -Fc -- "$1" "$WF")" -ge "$2" ]; }

echo "── Release Desktop macOS runner 门禁"
chk "matrix 只有一条 macos-latest（运行时一个 macOS job）" count_eq 'platform: macos-latest' 1
chk "不存在第二条 macOS matrix 目标" bash -c "[ \"\$(grep -Ec '^[[:space:]]+- platform: macos-' '$WF')\" -eq 1 ]"
chk "arm64 目标在 workflow 中出现一次" count_eq 'args: --target aarch64-apple-darwin' 1
chk "x86_64 目标在 workflow 中出现一次" count_eq 'args: --target x86_64-apple-darwin' 1
chk "tauri-action 至少三条（macOS 两条 + 非 macOS 一条）" count_ge 'uses: tauri-apps/tauri-action@' 3
chk "macOS 构建步骤限定 runner.os == macOS" count_ge "if: runner.os == 'macOS'" 2
chk "非 macOS 步骤使用 matrix.args" has 'args: ${{ matrix.args }}'
chk "保留 workflow_dispatch 重跑能力" has 'workflow_dispatch:'
chk "保留 publish-to-hub 回挂作业" has 'publish-to-hub:'
chk "保留 hub train-advance 回调" has 'train-advance'

if [ "$FAIL" -ne 0 ]; then
    echo "✗ Release Desktop macOS runner 门禁失败" >&2
    exit 1
fi
echo "✓ Release Desktop macOS runner 门禁通过"