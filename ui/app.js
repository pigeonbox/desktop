    const { invoke } = window.__TAURI__.core;
    const $ = id => document.getElementById(id);

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
      $('url').addEventListener('keydown', e => { if (e.key === 'Enter') connect(); });
      $('p2p-code').addEventListener('keydown', e => { if (e.key === 'Enter') recv(); });
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

    async function pickAndSend() {
      resetP2p();
      const path = await window.__TAURI__.dialog.open({ multiple: false });
      if (!path) return;
      // 口令由本端生成并立即展示(经 --code 下发给 p2pc)——CLI 自动口令只在传完才打印
      const code = genCode();
      $('p2p-mycode').textContent = code;
      $('p2p-mycode').style.display = 'block';
      start('send', { filePath: path, code, ...txOpts() });
    }
    function txOpts() {
      return {
        mode: $('p2p-mode') ? $('p2p-mode').value : 'auto',
        relay: $('p2p-relay') ? $('p2p-relay').value.trim() : '',
      };
    }
    function toggleAdv() {
      const adv = $('p2p-adv');
      const open = adv.style.display !== 'none';
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
          $('p2p-mycode').style.display = 'block';
        }
      } else if (p.kind === 'stderr' || p.kind === 'error') {
        log.textContent += p.line + '\n';
        log.scrollTop = log.scrollHeight;
      } else if (p.kind === 'exit') {
        transferDone = true;
        setBusy(false);
        if (p.code === 0) {
          $('p2p-mycode').classList.add('ok');
          if (!$('p2p-mycode').textContent) $('p2p-mycode').textContent = '✓ 传输完成';
          if ($('p2p-mycode').textContent.startsWith('✓') === false) $('p2p-mycode').textContent += ' ✓';
        } else {
          $('p2p-err').textContent = '传输异常退出（code ' + p.code + '），详见日志';
        }
      }
    }
    function resetP2p() {
      $('p2p-err').textContent = '';
      $('p2p-mycode').textContent = '';
      $('p2p-mycode').style.display = 'none';
      $('p2p-mycode').classList.remove('ok');
      $('p2p-log').textContent = '';
      $('p2p-log').style.display = 'none';
      $('p2p-path').style.display = 'none';
    }
    function setBusy(busy, label) {
      for (const id of ['pick', 'recv', 'p2p-code', 'p2p-registry']) $(id).disabled = busy;
      $('p2p-cancel').style.display = busy ? 'block' : 'none';
      if (busy) $('p2p-cancel').textContent = label || '取消传输';
    }