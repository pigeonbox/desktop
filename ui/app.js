    const { invoke } = window.__TAURI__.core;
    const $ = id => document.getElementById(id);

    // 事件必须走 addEventListener:CSP(default-src 'self')连内联 onclick/onchange
    // 一并拦截(内联处理器=内联脚本),曾致所有点击静默失效而键盘监听正常。
    (function bindEvents() {
      const bind = (id, ev, fn) => { const el = $(id); if (el) el.addEventListener(ev, fn); };
      bind('go', 'click', connect);
      bind('url', 'keydown', e => { if (e.key === 'Enter') connect(); });
      bind('save-key', 'click', saveKey);
      bind('clear-key', 'click', clearKey);
      bind('check-now', 'click', () => checkNow(true));
      bind('scope', 'change', onScopeChange);
    })();

    // 从托盘「服务器设置」进来时带 manual=1 ⇒ **不自动重连**。
    // 理由：自动重连会把页面导航回远端服务器,设置页(含取件待办)将永远站不住
    // ——用户既改不了服务器,也配不了 API Key。
    const MANUAL = new URLSearchParams(location.search).get('manual') === '1';

    window.addEventListener('DOMContentLoaded', async () => {
      // 服务器连接(原有逻辑)
      const saved = await invoke('get_saved');
      if (saved) {
        $('url').value = saved;
        if (!MANUAL) {
          try { await connect(); } catch (_) {}
          $('go').disabled = false;
          // 自动重连后页面已是远端服务器 ⇒ 取件待办面板随之消失(下方的
          // 取件面板只在本地设置页里有意义)。
          return;
        }
      }
      $('go').disabled = false;
      $('url').focus();
      initPickup();
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

    // ==================== 取件待办（一等取件端） ====================

    function initPickup() {
      if (!$('pickup-status')) return;
      $('scope').value = 'delivered';
      refreshPickup();
    }

    async function refreshPickup() {
      let st;
      try {
        st = await invoke('get_pickup_status');
      } catch (e) {
        $('pickup-status').textContent = '读取状态失败：' + String(e);
        return;
      }
      $('scope').value = st.scope || 'delivered';
      renderStatus(st);
      renderPending(st.pending || []);
    }

    function renderStatus(st) {
      const el = $('pickup-status');
      el.textContent = '';
      const line = (text, cls) => {
        const s = document.createElement('span');
        s.textContent = text;
        if (cls) s.className = cls;
        el.appendChild(s);
      };
      const n = st.pending_count || 0;
      line('待取 ', null);
      const b = document.createElement('b');
      b.textContent = String(n);
      el.appendChild(b);
      line(' 件', null);
      if (st.configured) {
        line(' · API Key 已配置', null);
      } else {
        line(' · 未配置 API Key（取件待办不启用）', 'warn');
      }
      if (st.last_checked) {
        const secs = Math.max(0, Math.floor(Date.now() / 1000) - st.last_checked);
        line(' · 最后检查：' + ago(secs), null);
      }
      if (st.last_error) {
        const w = document.createElement('span');
        w.className = 'warn';
        w.textContent = ' · ' + st.last_error;
        el.appendChild(w);
      }
      if (st.last_copied) {
        line(' · 上次复制：' + st.last_copied, null);
      }
    }

    function ago(secs) {
      if (secs < 60) return '刚刚';
      if (secs < 3600) return Math.floor(secs / 60) + ' 分钟前';
      if (secs < 86400) return Math.floor(secs / 3600) + ' 小时前';
      return Math.floor(secs / 86400) + ' 天前';
    }

    function renderPending(items) {
      const box = $('pickup-list');
      box.textContent = '';
      if (!items.length) {
        const e = document.createElement('div');
        e.className = 'pickup-empty';
        e.textContent = '暂无待取件。';
        box.appendChild(e);
        return;
      }
      items.forEach(item => {
        const row = document.createElement('div');
        row.className = 'pickup-item';

        const code = document.createElement('span');
        code.className = 'pickup-code';
        code.textContent = item.pickup_code;
        row.appendChild(code);

        const name = document.createElement('span');
        name.className = 'pickup-name';
        name.textContent = item.file_name || '文本分享';
        row.appendChild(name);

        const tag = document.createElement('span');
        tag.className = 'pickup-tag';
        tag.textContent = item.delivered ? '投递到件' : '未取走';
        row.appendChild(tag);

        const btn = document.createElement('button');
        btn.className = 'btn-ghost';
        btn.textContent = '复制取件码';
        btn.addEventListener('click', () => copyCode(item.pickup_code));
        row.appendChild(btn);

        box.appendChild(row);
      });
    }

    async function copyCode(code) {
      $('key-err').textContent = '';
      try {
        await invoke('copy_pickup_code', { code });
        await refreshPickup();
      } catch (e) {
        $('key-err').textContent = String(e);
      }
    }

    async function saveKey() {
      $('key-err').textContent = '';
      const key = $('apikey').value.trim();
      if (!key) {
        $('key-err').textContent = '请先粘贴 API Key';
        return;
      }
      try {
        await invoke('save_api_key', { key });
        $('apikey').value = '';
        await refreshPickup();
      } catch (e) {
        $('key-err').textContent = String(e);
      }
    }

    async function clearKey() {
      $('key-err').textContent = '';
      try {
        await invoke('clear_api_key');
        $('apikey').value = '';
        await refreshPickup();
      } catch (e) {
        $('key-err').textContent = String(e);
      }
    }

    async function onScopeChange() {
      $('key-err').textContent = '';
      try {
        await invoke('set_pickup_scope', { scope: $('scope').value });
        await refreshPickup();
      } catch (e) {
        $('key-err').textContent = String(e);
      }
    }

    async function checkNow(manual) {
      $('key-err').textContent = '';
      if (manual) $('pickup-status').textContent = '检查中…';
      try {
        const st = await invoke('check_pickup_now');
        renderStatus(st);
        renderPending(st.pending || []);
      } catch (e) {
        $('pickup-status').textContent = '检查失败：' + String(e);
      }
    }
