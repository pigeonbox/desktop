    const { invoke } = window.__TAURI__.core;
    const $ = id => document.getElementById(id);

    // 事件必须走 addEventListener:CSP(default-src 'self')连内联 onclick/onchange
    // 一并拦截(内联处理器=内联脚本),曾致所有点击静默失效而键盘监听正常。
    (function bindEvents() {
      const bind = (id, ev, fn) => { const el = $(id); if (el) el.addEventListener(ev, fn); };
      bind('go', 'click', connect);
      bind('pick', 'click', pickAndSend);
      bind('recv', 'click', recv);
      bind('p2p-cancel', 'click', cancel);
      bind('p2p-adv-toggle', 'click', toggleAdv);
      bind('p2p-mode', 'change', e => localStorage.setItem('p2p-mode', e.target.value));
      bind('p2p-relay', 'change', e => localStorage.setItem('p2p-relay', e.target.value));
      bind('p2p-copy', 'click', copyMyCode);
      bind('p2p-pair-toggle', 'click', togglePairMode);
      bind('p2p-accept', 'click', () => decidePair(true));
      bind('p2p-deny', 'click', () => decidePair(false));
      bind('url', 'keydown', e => { if (e.key === 'Enter') connect(); });
      bind('p2p-code', 'keydown', e => { if (e.key === 'Enter') recv(); });
    })();

    // 口令复制（clipboard API 优先，webview 不可用时回退选区复制）
    async function copyMyCode() {
      const code = $('p2p-mycode').textContent.trim();
      if (!code) return;
      try {
        await navigator.clipboard.writeText(code);
      } catch {
        const range = document.createRange();
        range.selectNode($('p2p-mycode'));
        const sel = getSelection();
        sel.removeAllRanges(); sel.addRange(range);
        document.execCommand('copy');
        sel.removeAllRanges();
      }
      $('p2p-copy').textContent = '✓ 已复制';
      setTimeout(() => { $('p2p-copy').textContent = '复制口令'; }, 1500);
    }
    // 口令块显隐统一走 wrap（含复制按钮与对端指引）
    function showMyCode(on) { $('p2p-mycode-wrap').hidden = !on; }

    window.addEventListener('DOMContentLoaded', async () => {
      // 传输模式/自定义中继:恢复持久化
      if ($('p2p-mode')) $('p2p-mode').value = localStorage.getItem('p2p-mode') || 'auto';
      if ($('p2p-relay')) $('p2p-relay').value = localStorage.getItem('p2p-relay') || '';
      // 设备直传面板初始化(独立于服务器连接)
      try {
        $('p2p-registry').value = await invoke('get_p2p_registry');
        await invoke('p2p_version'); // 探测 sidecar,失败则提示
      } catch (e) {
        $('p2p-note').textContent = '设备直传不可用：内置 p2pc 缺失（' + String(e).slice(0, 80) + '）';
        for (const id of ['pick', 'recv', 'p2p-code', 'p2p-registry']) $(id).disabled = true;
      }
      const { listen } = window.__TAURI__.event;
      await listen('p2p-log', ev => onP2pLog(ev.payload));

      // 服务器连接(原有逻辑)
      const saved = await invoke('get_saved');
      if (saved) {
        $('url').value = saved;
        try { await connect(); return; } catch (_) {}
        $('go').disabled = false;
      }
      $('url').focus();
    });

    async function connect() {
      let url = $('url').value.trim();
      // 明文 HTTP 确认（2026-10-05 审计：无 scheme 时 Rust 侧默认补 http://，
      // 内网 NAS 场景保留该便利，但须用户知情——凭据与分享内容明文传输）
      const target = url.startsWith('http://') ? url
        : (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url) && !/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/.test(url)) ? 'http://' + url
        : null;
      const eff = target || url;
      if (eff.startsWith('http://')) {
        const host = eff.replace(/^http:\/\//, '').split(/[\/ :]/)[0];
        // 内网/回环免确认:主机名精确匹配,或私网段前缀
        // (2026-10-11 修复:原模式尾部误留一个字面空格,导致 10./192.168. 等
        // 内网地址永远判不中,每个 http:// 地址都弹明文确认框)
        const local = /^(localhost|127\.0\.0\.1|\[::1\])$/.test(host) ||
          /^(192\.168\.|10\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host);
        if (!local && !confirm('将以明文 HTTP 连接（内容与凭据不加密传输）。\n' + eff + '\n\n继续连接吗？')) {
          return;
        }
      }
      $('err').textContent = '';
      $('go').disabled = true;
      try {
        await invoke('connect', { url });
      } catch (e) {
        $('err').textContent = String(e);
        $('go').disabled = false;
      }
    }

    // ── 设备直传 ──
    let transferDone = true;
    let lastErr = '';
    // 配对码模式(2026-10-18):发送侧亮 4 位码+接受门;事件经 --json NDJSON
    // 解析(见 onPairEvent),裁决经 p2p_pair_approve 写子进程 stdin。
    let pairMode = false;
    let pairTimer = null;

    function togglePairMode() {
      if (!transferDone) return;
      pairMode = !pairMode;
      $('p2p-pair-toggle').textContent = pairMode
        ? '改回口令发送（12 位口令，适合异步转交）'
        : '改用 4 位配对码（对方输码即连，免抄口令）';
      $('p2p-send-hint').textContent = pairMode
        ? '（亮 4 位码，对方 5 分钟内输码即连，需你确认）'
        : '（选好后自动生成口令，发给对方即可）';
    }

    async function pickAndSend() {
      resetP2p();
      const path = await window.__TAURI__.dialog.open({ multiple: false });
      if (!path) return;
      if (pairMode) {
        // 配对码由 p2pc 铸出,经 pair_code 事件回传后展示
        start('send', { filePath: path, code: '', pair: true, ...txOpts() });
        return;
      }
      // 口令由本端生成并立即展示(经 --code 下发给 p2pc)——CLI 自动口令只在传完才打印
      const code = genCode();
      $('p2p-mycode-label').textContent = '把口令发给对方（10 分钟内有效，一次性）';
      $('p2p-mycode-tip').textContent = '对方在 PigeonBox 桌面版「设备直传 → 接收」贴入口令，或在对方电脑执行 p2pc recv 口令';
      $('p2p-copy').textContent = '复制口令';
      $('p2p-mycode').textContent = code;
      showMyCode(true);
      start('send', { filePath: path, code, pair: false, ...txOpts() });
    }
    function txOpts() {
      return {
        mode: $('p2p-mode') ? $('p2p-mode').value : 'auto',
        relay: $('p2p-relay') ? $('p2p-relay').value.trim() : '',
      };
    }
    function toggleAdv() {
      const adv = $('p2p-adv');
      const open = adv.style.display === 'block';
      adv.style.display = open ? 'none' : 'block';
    }
    function genCode() {
      const cs = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
      const b = new Uint8Array(12);
      crypto.getRandomValues(b);
      const s = Array.from(b, x => cs[x % cs.length]).join('');
      return s.slice(0, 4) + '-' + s.slice(4, 8) + '-' + s.slice(8, 12);
    }
    async function recv() {
      resetP2p();
      const code = $('p2p-code').value.trim();
      if (!code) { $('p2p-err').textContent = '请输入口令'; return; }
      start('recv', { code, ...txOpts() });
    }
    async function start(cmd, extra) {
      const registry = $('p2p-registry').value.trim();
      if (!registry) { $('p2p-err').textContent = '请填写直传注册中心地址'; return; }
      $('p2p-err').textContent = '';
      setBusy(true, cmd === 'send' ? '发送中…' : '接收中…');
      try {
        await invoke(cmd === 'send' ? 'p2p_send' : 'p2p_recv', { registry, ...extra });
        transferDone = false;
      } catch (e) {
        $('p2p-err').textContent = String(e);
        setBusy(false);
      }
    }
    async function cancel() {
      try { await invoke('p2p_cancel'); } catch (_) {}
    }

    function onP2pLog(p) {
      const log = $('p2p-log');
      log.style.display = 'block';
      if (p.kind === 'stdout') {
        log.textContent += p.line + '\n';
        log.scrollTop = log.scrollHeight;
        // --json NDJSON 行(pairing/recv-pairing 模式)→ 结构化配对事件
        const t = p.line.trim();
        if (t.startsWith('{') && t.includes('"event"')) {
          try { onPairEvent(JSON.parse(t)); } catch (_) {}
          return;
        }
        // 传输路径徽章(via=quic-direct/relay)
        const vm = p.line.match(/via=([a-z0-9()+-]+)/);
        if (vm) {
          const chip = $('p2p-path');
          const direct = vm[1].indexOf('quic-direct') === 0;
          chip.textContent = direct ? '当前路径：直连 ' + vm[1] : '当前路径：中继兜底';
          chip.className = 'path-chip ' + (direct ? 'direct' : 'relay');
          chip.style.display = 'block';
        }
        // send 会打印口令 XXXX-XXXX-XXXX,提取后大字展示
        const m = p.line.match(/\b([A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4})\b/);
        if (m && !transferDone && !$('p2p-mycode').textContent) {
          $('p2p-mycode').textContent = m[1];
          showMyCode(true);
        }
      } else if (p.kind === 'stderr' || p.kind === 'error') {
        log.textContent += p.line + '\n';
        log.scrollTop = log.scrollHeight;
        // 失败即上屏:错误行实时进错误框,不让失败静默
        if (/失败|不可用|超时|timeout|EOF|refused|错误|不存在|仅直连|打洞|拒绝|invalid|not found|limit/i.test(p.line)) {
          lastErr = p.line;
          $('p2p-err').textContent = '\u2716 \u4f20\u8f93\u5f02\u5e38\uff1a' + p.line.trim();
        }
      } else if (p.kind === 'exit') {
        transferDone = true;
        setBusy(false);
        if (p.code === 0) {
          $('p2p-mycode').classList.add('ok');
          if (!$('p2p-mycode').textContent) $('p2p-mycode').textContent = '✓ 传输完成';
          if ($('p2p-mycode').textContent.startsWith('✓') === false) $('p2p-mycode').textContent += ' ✓';
        } else {
          // 失败明示:优先展示具体原因行,其次退出码(不让失败停在日志里)
          const why = lastErr ? lastErr.trim() : ('异常退出 code ' + p.code + '，详见下方日志');
          $('p2p-err').textContent = '\u2716 \u4f20\u8f93\u5931\u8d25\uff1a' + why;
          $('p2p-path').textContent = '\u4f20\u8f93\u5931\u8d25';
          $('p2p-path').className = 'path-chip fail';
          $('p2p-path').style.display = 'block';
        }
      }
    }
    function resetP2p() {
      $('p2p-err').textContent = '';
      $('p2p-mycode').textContent = '';
      showMyCode(false);
      $('p2p-mycode').classList.remove('ok');
      $('p2p-log').textContent = '';
      $('p2p-log').style.display = 'none';
      lastErr = '';
      $('p2p-path').style.display = 'none';
      $('p2p-pair-card').hidden = true;
      setHint('');
      if (pairTimer) { clearInterval(pairTimer); pairTimer = null; }
    }

    // ── 配对码事件(--json NDJSON) ──
    let pairDeadline = 0;
    function onPairEvent(ev) {
      switch (ev.event) {
        case 'pair_code': {
          showMyCode(true);
          $('p2p-mycode-label').textContent = '配对码（对方输码即连，需你确认）';
          $('p2p-mycode-tip').textContent = '对方在「接收文件」输入此 4 位码，或执行 p2pc recv ' + ev.code;
          $('p2p-copy').textContent = '复制配对码';
          $('p2p-mycode').textContent = ev.code;
          pairDeadline = ev.ttl_seconds > 0 ? Date.now() + ev.ttl_seconds * 1000 : 0;
          tickPairCountdown();
          break;
        }
        case 'pair_request': {
          $('p2p-pair-from').textContent = ev.name || String(ev.from || '').slice(0, 8) || '未知设备';
          $('p2p-pair-card').hidden = false;
          break;
        }
        case 'pair_denied':
        case 'pair_expired': {
          $('p2p-pair-card').hidden = true;
          setHint(ev.event === 'pair_denied' ? '已拒绝，正在生成新配对码…' : '配对码已过期，正在生成新码…');
          break;
        }
        case 'pair_pending': {
          setHint('已认领配对码，等待对方确认…');
          break;
        }
      }
    }
    function tickPairCountdown() {
      if (pairTimer) { clearInterval(pairTimer); pairTimer = null; }
      if (!pairDeadline) return;
      pairTimer = setInterval(() => {
        if (transferDone || !$('p2p-mycode').textContent) { clearInterval(pairTimer); pairTimer = null; return; }
        const left = Math.max(0, Math.round((pairDeadline - Date.now()) / 1000));
        setHint('配对码 ' + $('p2p-mycode').textContent + ' · 剩余 ' + fmtDurPair(left) + ' · 过期自动换新码');
      }, 500);
    }
    function fmtDurPair(s) {
      const m = Math.floor(s / 60), r = s % 60;
      return m + ':' + String(r).padStart(2, '0');
    }
    function setHint(text) {
      const el = $('p2p-hint');
      if (!el) return;
      el.textContent = text;
      el.style.display = text ? 'block' : 'none';
    }
    async function decidePair(accept) {
      $('p2p-pair-card').hidden = true;
      try { await invoke('p2p_pair_approve', { accept }); } catch (e) {
        $('p2p-err').textContent = String(e);
      }
      if (!accept) setHint('已拒绝，正在生成新配对码…');
    }
    function setBusy(busy, label) {
      for (const id of ['pick', 'recv', 'p2p-code', 'p2p-registry']) $(id).disabled = busy;
      $('p2p-cancel').style.display = busy ? 'block' : 'none';
      if (busy) $('p2p-cancel').textContent = label || '取消传输';
    }