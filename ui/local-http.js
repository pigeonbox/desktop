// 明文 HTTP 连接的「内网免确认」判定（#145）。
//
// 这是凭据明文传输的**唯一用户知情闸**（Rust 侧 normalize_url 对 http 不设防）。
// 判定口径：
//   - 用 URL 解析取 host（字符串切分会把 http://[::1] 拆碎 ⇒ IPv6 回环误弹）；
//   - 私网判定只认 **IP 字面量**——域名形态（10.evil.com）一律不免确认；
//   - IPv4：回环 127.0.0.0/8 + 私网 10/8、172.16/12、192.168/16；
//   - IPv6：回环 ::1 + ULA fc00::/7 + 链路本地 fe80::/10。
//
// 本文件独立成模块：浏览器经 <script> 挂到 window，Node 侧可 require 单测
// （ui/local-http.test.js，CI 的「前端判定函数单测」步骤跑）。
(function (global) {
  'use strict';

  /// 是否 IPv4 字面量（4 段十进制且各段 ≤255）；不是则返回 null。
  function parseIPv4(host) {
    const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
    if (!m) return null;
    const parts = m.slice(1).map(Number);
    return parts.every((p) => p <= 255) ? parts : null;
  }

  /// IPv4 字面量是否回环/私网。
  function ipv4IsLocal(parts) {
    const [a, b] = parts;
    if (a === 127) return true; // 回环 127.0.0.0/8
    if (a === 10) return true; // 私网 10.0.0.0/8
    if (a === 172 && b >= 16 && b <= 31) return true; // 私网 172.16.0.0/12
    if (a === 192 && b === 168) return true; // 私网 192.168.0.0/16
    return false;
  }

  /// IPv6 字面量是否回环/私网（URL 的 hostname 对 IPv6 带方括号，调用前已剥）。
  function ipv6IsLocal(host) {
    const h = host.toLowerCase();
    if (h === '::1') return true; // 回环
    if (/^f[cd]/.test(h)) return true; // ULA fc00::/7
    if (/^fe[89ab]/.test(h)) return true; // 链路本地 fe80::/10
    if (h.startsWith('::ffff:')) {
      const v4 = parseIPv4(h.slice(7)); // IPv4-mapped（::ffff:10.0.0.1）按 v4 判
      return v4 ? ipv4IsLocal(v4) : false;
    }
    return false;
  }

  /// host 是否免确认的内网 IP 字面量/回环主机名；域名一律 false。
  function isLocalHost(hostname) {
    if (hostname === 'localhost') return true;
    const v4 = parseIPv4(hostname);
    if (v4) return ipv4IsLocal(v4);
    if (hostname.includes(':')) return ipv6IsLocal(hostname);
    return false; // 域名（含 10.evil.com 这类私网前缀公网域名）一律不免确认
  }

  // 是否需要弹「明文 HTTP」确认框。
  //   - 非 http:// 开头 ⇒ 不需要（调用方也只在明文场景调）；
  //   - http 形态但 URL 解析不了 ⇒ 保守要确认；
  //   - 解析成功 ⇒ host 是免确认内网 IP 字面量则免，否则要确认。
  function needsClearHttpConfirm(eff) {
    if (!/^http:\/\//i.test(eff)) return false;
    let u;
    try {
      u = new URL(eff);
    } catch (_) {
      return true;
    }
    if (u.protocol !== 'http:') return false;
    // WHATWG URL 的 IPv6 hostname 带方括号（[::1]），剥掉再判。
    const hostname = u.hostname.replace(/^\[|\]$/g, '');
    return !isLocalHost(hostname);
  }

  global.needsClearHttpConfirm = needsClearHttpConfirm;
  if (typeof module === 'object' && module.exports) {
    module.exports = { needsClearHttpConfirm };
  }
})(typeof window !== 'undefined' ? window : globalThis);
