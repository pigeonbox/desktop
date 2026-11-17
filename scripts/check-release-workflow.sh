#!/bin/bash
# Release Desktop 工作流门禁（desktop#156 / #157）。
#
# #156：macos-latest 落到 arm64 镜像后，原工作流把 aarch64/x86_64 拆成两个
# job ⇒ 一趟发布要同时抢到两个 arm64 runner；容量紧张时必有一条腿排队超时
# 被取消，publish-to-hub 随之跳过。修复=PASS：两个目标合进同一 job 顺序构建。
#
# #157：同 tag 多跑 + --clobber + macOS 非可复现 ⇒ 已发布资产被静默换字节，
# hub manifest sha 漂移。修复=PASS：
#   - 同 tag concurrency 串行；
#   - preflight 读 release-complete.json（tag commit + 全资产 sha256），
#     已完成则跳过整个重建；
#   - finalize 在所有平台成功后写完成标记；
#   - publish-to-hub 先按标记校验本地字节，再按 hub 摘要决定 skip/upload，
#     只有摘要不一致（显式告警）才 --clobber；
#   - macOS 构建带 SOURCE_DATE_EPOCH / ZERO_AR_DATE 等可复现标志。
#
# 本门禁保证：macOS runner 合并、幂等跳过、完成标记、摘要校验、
# 可复现标志、回挂与 dispatch 能力任一被改掉即红。
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

echo "── Release Desktop 发布幂等/可复现门禁（#157）"
chk "同 tag 并发串行（concurrency + cancel-in-progress:false）" bash -c "grep -Fq 'group: \${{ github.workflow }}-\${{ inputs.version || github.ref_name }}' '$WF' && grep -Fq 'cancel-in-progress: false' '$WF'"
chk "存在 preflight 复跑幂等闸" has 'preflight:'
chk "preflight 输出 skip 并默认 false" bash -c "grep -Fq 'skip: \${{ steps.check.outputs.skip }}' '$WF' && grep -Fq 'echo \"skip=false\"' '$WF'"
chk "已完成（skip=true）时跳过 build" has "if: needs.preflight.outputs.skip != 'true'"
chk "完成标记 release-complete.json" has 'release-complete.json'
chk "完成标记含 tag commit 校验" has '完成标记 commit='
chk "完成标记含资产 sha256 校验" bash -c "grep -Fq '\"sha256\"' '$WF' && grep -Fq 'sha256sum' '$WF'"
chk "finalize 在全部构建成功后写标记" has 'finalize:'
chk "回挂前校验本地资产与标记一致" has '发布后被替换？'
chk "hub 回挂按摘要比较，不再无条件 clobber" bash -c "grep -Fq 'remote_digest' '$WF' && grep -Fq '摘要一致，跳过' '$WF'"
chk "摘要不一致显式告警后才 clobber 修复" has '摘要不一致'
chk "macOS 可复现时间戳 SOURCE_DATE_EPOCH" has 'SOURCE_DATE_EPOCH: ${{ steps.repro.outputs.epoch }}'
chk "可复现归档标志 ZERO_AR_DATE" has 'ZERO_AR_DATE: "1"'

if [ "$FAIL" -ne 0 ]; then
    echo "✗ Release Desktop 工作流门禁失败" >&2
    exit 1
fi
echo "✓ Release Desktop 工作流门禁通过"