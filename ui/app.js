    const { invoke } = window.__TAURI__.core;
    const $ = id => document.getElementById(id);

    // 事件必须走 addEventListener:CSP(default-src 'self')连内联 onclick/onchange
    // 一并拦截(内联处理器=内联脚本),曾致所有点击静默失效而键盘监听正常。
    (function bindEvents() {
      const bind = (id, ev, fn) => { const el = $(id); if (el) el.addEventListener(ev, fn); };
      bind('go', 'click', connect);
      bind('url', 'keydown', e => { if (e.key === 'Enter') connect(); });
    })();

    window.addEventListener('DOMContentLoaded', async () => {
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
      // scheme 大小写不敏感（HTTP:// 与 http:// 同义，2026-10-26 审计 P1）：
      // 此前用 startsWith('http://') 判大小写敏感 ⇒ "HTTP://box.local" 走明文
      // 却弹不出确认框。口径与 Rust 侧 normalize_url / has_scheme 对齐。
      const httpScheme = /^http:\/\//i;
      const target = httpScheme.test(url) ? url
        : (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url) && !/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/.test(url)) ? 'http://' + url
        : null;
      const eff = target || url;
      if (httpScheme.test(eff)) {
        const host = eff.replace(/^http:\/\//i, '').split(/[\/ :]/)[0];
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
