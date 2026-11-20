#!/bin/bash
# shellcheck disable=SC2016
# ↑ 本脚本里出现的 ${{ ... }} 全部是**要在 workflow YAML 里逐字匹配的串**，
#   不是 shell 变量；单引号是刻意为之（双引号会被 shell 当成变量/命令替换）。
# Release Desktop 工作流门禁（desktop#156 / #157 / #173）。
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
# #173（6.10.0 实录 run 44680032391）：ensure-release 建骨架写成
# `gh release create --verify-tag --target "$TAG"`，而 GitHub API 的
# target_commitish **只接受分支名或 commit SHA**，tag 名被判非法 ⇒
# HTTP 422 "Release.target_commitish is invalid" ⇒ 骨架没建成 ⇒ 下游 build
# 被连带跳过 ⇒ 整条发布链断在骨架这一步（ensure-release failed /
# publish-to-hub failed / build、finalize 全 skipped ⇒ hub 侧 desktop 资产
# 永久缺失 ⇒ 列车 finalize 出不来生态快照）。
# 修法=PASS：
#   ① 去掉 --target（--verify-tag 已校验 tag 存在，且让 Release 指向该 tag）；
#   ② 骨架建不出来时**立刻失败**并给出可操作命令，而不是「只告警」后让
#      4 个平台构建白跑（旧写法的 else 分支最后一条命令 gh release view
#      返回非 0，在 set -e 下反而硬失败 ⇒ 注释与实现自相矛盾）。
# ⚠️ 本门禁此前把 `--verify-tag --target` 当成**正确形态**钉死
#   （"ensure-release 创建前校验 tag 存在" 判据查的就是这个串）⇒ 门禁钉住
#   了坏形态，正是缺陷能长期存活的原因。⇒ 本次改为：ensure-release 段内
#   **禁止出现 --target**，并补行为级判据（骨架建不出来 ⇒ 必须非 0 退出）。
#
# 本门禁保证：macOS runner 合并、幂等跳过、完成标记、摘要校验、可复现标志、
# 回挂与 dispatch 能力任一被改掉即红；骨架形态与骨架失败语义由行为判据钉住。
#
# 用法：
#   bash scripts/check-release-workflow.sh              # 只跑静态门禁
#   bash scripts/check-release-workflow.sh --selfcheck  # 静态门禁 + 行为级自检
#                                                      # （正对照绿 / 负对照红）
set -euo pipefail

cd "$(dirname "$0")/.."
WF_DEFAULT=".github/workflows/release.yml"

# 从 workflow 里抽出 ensure-release 作业的 shell 步骤正文（去掉 YAML 缩进）。
extract_ensure_step() {
    awk '
      /^  ensure-release:/{f=1; next}
      f && /^  [a-z][a-z-]*:/{exit}
      f && /^        run: [|]/{g=1; next}
      g {
        if ($0 ~ /^          /) { sub(/^          /, ""); print; next }
        if ($0 ~ /^[[:space:]]*$/) { print ""; next }
        exit
      }
    ' "$1"
}

# mock gh：按 MOCK_MODE 决定 release create / release view 的行为。
#   create_ok     ⇒ create 成功、view 一律「不存在」
#   view_exists   ⇒ create 失败、view 第二次起「存在」（并发已建场景）
#   none          ⇒ create 失败、view 一律「不存在」（骨架建不出来场景）
write_mock_gh() {
    mkdir -p "$1"
    cat > "$1/gh" <<'MOCK'
#!/bin/bash
if [ "$1" = "release" ] && [ "$2" = "create" ]; then
  if [ "${MOCK_MODE:-}" = "create_ok" ]; then
    echo "https://github.com/mock/repo/releases/tag/mock"
    exit 0
  fi
  echo "HTTP 422: Validation Failed" >&2
  exit 1
fi
if [ "$1" = "release" ] && [ "$2" = "view" ]; then
  n=0
  [ -f "${MOCK_STATE:-/dev/null}" ] && n="$(cat "$MOCK_STATE")"
  n=$((n + 1))
  echo "$n" > "${MOCK_STATE:-/dev/null}"
  if [ "${MOCK_MODE:-}" = "view_exists" ] && [ "$n" -ge 2 ]; then
    exit 0
  fi
  exit 1
fi
exit 0
MOCK
    chmod +x "$1/gh"
}

run_gate() {
    local WF="$1"
    local FAIL=0

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
    # all_has：全部串都要命中（避免用 bash -c 嵌套 shell 解析 ${{ }} 与变量）。
    all_has()  { local s=0; for x in "$@"; do has "$x" || s=1; done; [ "$s" -eq 0 ]; }
    count_eq() { [ "$(grep -Fc -- "$1" "$WF")" -eq "$2" ]; }
    count_re_eq() { [ "$(grep -Ec -- "$1" "$WF")" -eq "$2" ]; }
    count_ge() { [ "$(grep -Fc -- "$1" "$WF")" -ge "$2" ]; }
    # block_has 在某个 job 段内查串（段 = 该 job 行到下一个 job 行之间）。
    block_has() { printf '%s' "$1" | grep -Fq -- "$2"; }
    # block_not_has：段内不得出现某串。
    block_not_has() { ! printf '%s' "$1" | grep -Fq -- "$2"; }
    # block_code_not_has：只看段内**可执行行**（注释里出现字样不算行为；
    # 但也不能靠注释藏行为——注释行一律剥离后判）。
    block_code_not_has() {
        if printf '%s\n' "$1" | grep -vE '^[[:space:]]*#' | grep -Fq -- "$2"; then
            return 1
        fi
        return 0
    }
    # block_code_has：段内可执行行命中某串。
    block_code_has() {
        printf '%s\n' "$1" | grep -vE '^[[:space:]]*#' | grep -Fq -- "$2"
    }

    echo "── Release Desktop macOS runner 门禁"
    chk "matrix 只有一条 macos-latest（运行时一个 macOS job）" count_eq 'platform: macos-latest' 1
    chk "不存在第二条 macOS matrix 目标" count_re_eq '^[[:space:]]+- platform: macos-' 1
    chk "arm64 目标在 workflow 中出现一次" count_eq 'args: --target aarch64-apple-darwin' 1
    chk "x86_64 目标在 workflow 中出现一次" count_eq 'args: --target x86_64-apple-darwin' 1
    chk "tauri-action 至少三条（macOS 两条 + 非 macOS 一条）" count_ge 'uses: tauri-apps/tauri-action@' 3
    chk "macOS 构建步骤限定 runner.os == macOS" count_ge "if: runner.os == 'macOS'" 2
    chk "非 macOS 步骤使用 matrix.args" has 'args: ${{ matrix.args }}'
    chk "保留 workflow_dispatch 重跑能力" has 'workflow_dispatch:'
    chk "保留 publish-to-hub 回挂作业" has 'publish-to-hub:'
    chk "保留 hub train-advance 回调" has 'train-advance'

    echo "── Release Desktop 发布幂等/可复现门禁（#157）"
    chk "同 tag 并发串行（concurrency + cancel-in-progress:false）" all_has 'group: ${{ github.workflow }}-${{ inputs.version || github.ref_name }}' 'cancel-in-progress: false'
    chk "存在 preflight 复跑幂等闸" has 'preflight:'
    chk "preflight 输出 skip 并默认 false" all_has 'skip: ${{ steps.check.outputs.skip }}' 'echo "skip=false"'
    chk "已完成（skip=true）时跳过 build" has "if: needs.preflight.outputs.skip != 'true'"
    chk "完成标记 release-complete.json" has 'release-complete.json'
    chk "完成标记含 tag commit 校验" has '完成标记 commit='
    chk "完成标记含资产 sha256 校验" all_has '"sha256"' 'sha256sum'
    chk "finalize 在全部构建成功后写标记" has 'finalize:'
    chk "回挂前校验本地资产与标记一致" has '发布后被替换？'
    chk "hub 回挂按摘要比较，不再无条件 clobber" all_has 'remote_digest' '摘要一致，跳过'
    chk "摘要不一致显式告警后才 clobber 修复" has '摘要不一致'
    chk "macOS 可复现时间戳 SOURCE_DATE_EPOCH" has 'SOURCE_DATE_EPOCH: ${{ steps.repro.outputs.epoch }}'
    chk "可复现归档标志 ZERO_AR_DATE" has 'ZERO_AR_DATE: "1"'

    # #162：tag push 触发时本仓 Release 不存在 ⇒ tauri-action 走 runtime-create ⇒
    # GITHUB_TOKEN 无权 create-a-release ⇒ 四平台 job 全红在最后一步（6.1.0 实录
    # run 43990425854）。修法=先用有写权限的 PAT 建骨架，让 tauri-action 走 upload。
    echo "── Release Desktop Release 骨架门禁（#162）"
    ensure_block="$(awk '/^  ensure-release:/{f=1;next} /^  [a-z][a-z-]*:/{f=0} f' "$WF")"
    chk "存在 ensure-release 作业（骨架先于构建）" has 'ensure-release:'
    chk "build 依赖 ensure-release" has 'needs: [preflight, ensure-release]'
    chk "ensure-release 段内用 gh release create 建骨架" block_has "$ensure_block" 'gh release create'
    chk "ensure-release 创建前校验 tag 存在（--verify-tag）" block_has "$ensure_block" '--verify-tag'
    chk "ensure-release 建骨架幂等（已存在则跳过）" block_has "$ensure_block" 'gh release view'
    chk "失败提示可操作（含手工建骨架命令）" block_has "$ensure_block" '请先手工建骨架'
    chk "组件 Release 不抢 Latest" block_has "$ensure_block" '--latest=false'

    # #173：--target 把 tag 名当 commitish ⇒ API 422 ⇒ 骨架建不出来 ⇒ 整条链断。
    echo "── Release Desktop 骨架形态门禁（#173）"
    chk "ensure-release 可执行行不得出现 --target（tag 名不是合法 commitish ⇒ API 422）" block_code_not_has "$ensure_block" '--target'
    chk "ensure-release 段内不得出现 ::warning::自动创建 Release 失败（骨架建不出来必须阻断）" block_not_has "$ensure_block" '::warning::自动创建 Release 失败'
    chk "骨架建不出来时必须非 0 退出（exit 1）" block_code_has "$ensure_block" 'exit 1'

    if [ "$FAIL" -ne 0 ]; then
        echo "✗ Release Desktop 工作流门禁失败" >&2
        return 1
    fi
    echo "✓ Release Desktop 工作流门禁通过"
    return 0
}

# 行为级自检：把 ensure-release 的 shell 抽出来，用 mock gh 跑三种场景。
behavior_selfcheck() {
    local WF="$1"
    local tmp rc
    tmp="$(mktemp -d)"

    extract_ensure_step "$WF" > "$tmp/step.sh"
    if [ ! -s "$tmp/step.sh" ]; then
        echo "✗ 行为自检：未能从 workflow 抽出 ensure-release 的 shell 步骤（结构已变？）" >&2
        rm -rf "$tmp"
        return 1
    fi
    write_mock_gh "$tmp/bin"

    # ① 骨架建得出来 ⇒ 步骤必须成功退出。
    rc=0
    MOCK_MODE=create_ok MOCK_STATE="$tmp/count1" PATH="$tmp/bin:$PATH" TAG=desktop-vTEST GITHUB_REPOSITORY=mock/repo bash "$tmp/step.sh" > "$tmp/out1" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "✗ 行为自检：骨架建得出来时步骤却失败（rc=$rc）" >&2
        sed -n '1,40p' "$tmp/out1" >&2
        rm -rf "$tmp"
        return 1
    fi
    echo "  ✓ 行为：骨架建得出来 ⇒ 步骤成功退出"

    # ② create 失败但复查发现 Release 已在（并发已建）⇒ 幂等视为成功。
    rc=0
    MOCK_MODE=view_exists MOCK_STATE="$tmp/count2" PATH="$tmp/bin:$PATH" TAG=desktop-vTEST GITHUB_REPOSITORY=mock/repo bash "$tmp/step.sh" > "$tmp/out2" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "✗ 行为自检：并发已建场景（create 失败 / view 存在）未按幂等成功处理（rc=$rc）" >&2
        sed -n '1,40p' "$tmp/out2" >&2
        rm -rf "$tmp"
        return 1
    fi
    echo "  ✓ 行为：create 失败但 Release 已存在 ⇒ 幂等成功"

    # ③ 骨架确实建不出来 ⇒ 必须非 0 退出（不能只告警后让 4 个平台构建白跑）。
    rc=0
    MOCK_MODE=none MOCK_STATE="$tmp/count3" PATH="$tmp/bin:$PATH" TAG=desktop-vTEST GITHUB_REPOSITORY=mock/repo bash "$tmp/step.sh" > "$tmp/out3" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "✗ 行为自检：骨架建不出来时步骤仍成功退出 ⇒ 会白跑 4 个平台构建" >&2
        rm -rf "$tmp"
        return 1
    fi
    if ! grep -Fq '请先手工建骨架' "$tmp/out3"; then
        echo "✗ 行为自检：骨架建不出来时未给出可操作命令" >&2
        sed -n '1,40p' "$tmp/out3" >&2
        rm -rf "$tmp"
        return 1
    fi
    echo "  ✓ 行为：骨架建不出来 ⇒ 非 0 退出且给出可操作命令"
    rm -rf "$tmp"
    return 0
}

case "${1:-}" in
    --selfcheck)
        # 正对照：真实 workflow 必须通过静态门禁。
        run_gate "$WF_DEFAULT" || {
            echo "✗ 正对照失败：真实 workflow 未通过静态门禁" >&2
            exit 1
        }
        # 正对照：真实 workflow 的 ensure-release 步骤必须满足三条行为不变量。
        behavior_selfcheck "$WF_DEFAULT" || exit 1

        # 负对照：注入已知坏形态（--target "$TAG"）⇒ 静态门禁必须判红。
        tmp="$(mktemp -d)"
        # 只注入**可执行行**（注释行不算行为，注入进去等于没注入）。
        awk '
          !done && index($0, "--verify-tag") && $0 !~ /^[[:space:]]*#/ {
            sub(/--verify-tag/, "--verify-tag --target \"$TAG\"")
            done = 1
          }
          { print }
        ' "$WF_DEFAULT" > "$tmp/bad.yml"
        if ! grep -Fq -- '--target "$TAG"' "$tmp/bad.yml"; then
            echo "✗ 负对照未能注入坏形态（workflow 结构已变，判据需同步）" >&2
            rm -rf "$tmp"
            exit 1
        fi
        if run_gate "$tmp/bad.yml" > "$tmp/gate.log" 2>&1; then
            echo "✗ 负对照失败：注入 --target 后门禁仍判绿 ⇒ 门禁没牙齿" >&2
            cat "$tmp/gate.log" >&2
            rm -rf "$tmp"
            exit 1
        fi
        echo "  ✓ 负对照：注入 --target 后门禁判红"
        echo "✓ Release Desktop 工作流门禁自检通过（正对照绿 / 负对照红）"
        rm -rf "$tmp"
        ;;
    *)
        run_gate "${1:-$WF_DEFAULT}"
        ;;
esac
