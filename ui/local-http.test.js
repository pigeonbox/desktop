// 明文 HTTP 确认的内网判据单测（#145）。
// 跑法：node --test ui/  （CI「前端判定函数单测」步骤同源）
const test = require('node:test');
const assert = require('node:assert');
const { needsClearHttpConfirm } = require('./local-http.js');

test('私网/回环 IP 字面量免确认（内网 NAS 便利不回退）', () => {
  assert.strictEqual(needsClearHttpConfirm('http://10.0.0.3'), false);
  assert.strictEqual(needsClearHttpConfirm('http://10.0.0.3:56789/x'), false);
  assert.strictEqual(needsClearHttpConfirm('http://192.168.1.5'), false);
  assert.strictEqual(needsClearHttpConfirm('http://172.16.0.1'), false);
  assert.strictEqual(needsClearHttpConfirm('http://172.31.255.1'), false);
  assert.strictEqual(needsClearHttpConfirm('http://127.0.0.1:8080'), false);
  assert.strictEqual(needsClearHttpConfirm('http://127.8.8.8'), false);
  assert.strictEqual(needsClearHttpConfirm('http://localhost:3000'), false);
  assert.strictEqual(needsClearHttpConfirm('HTTP://localhost'), false);
});

test('IPv6 回环/私网判得中（http://[::1] 不再误弹，#145）', () => {
  assert.strictEqual(needsClearHttpConfirm('http://[::1]'), false);
  assert.strictEqual(needsClearHttpConfirm('http://[::1]:8080/'), false);
  assert.strictEqual(needsClearHttpConfirm('http://[fd00::1]'), false);
  assert.strictEqual(needsClearHttpConfirm('http://[fe80::1]'), false);
});

test('私网前缀的公网域名一律不免确认（#145 绕过面）', () => {
  assert.strictEqual(needsClearHttpConfirm('http://10.evil.com'), true);
  assert.strictEqual(needsClearHttpConfirm('http://192.168.evil.com'), true);
  assert.strictEqual(needsClearHttpConfirm('http://172.16.x.tld'), true);
  assert.strictEqual(needsClearHttpConfirm('http://127.0.0.1.nip.io'), true);
  assert.strictEqual(needsClearHttpConfirm('http://box.local'), true);
});

test('公网明文地址必须确认，非 http 不需要', () => {
  assert.strictEqual(needsClearHttpConfirm('http://example.com'), true);
  assert.strictEqual(needsClearHttpConfirm('HTTP://example.com/path'), true);
  assert.strictEqual(needsClearHttpConfirm('https://example.com'), false);
  assert.strictEqual(needsClearHttpConfirm('box.local'), false);
  assert.strictEqual(needsClearHttpConfirm(''), false);
});
