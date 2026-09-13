#!/usr/bin/env node
/*
 * lan_crypto.js 与 Node 内置 aes-256-gcm 的双向交叉验证。
 * 运行：node scripts/lan-crypto-test.mjs
 *
 * Node 的 OpenSSL 实现作为标准答案：JS 加密 → Node 解密、Node 加密 → JS 解密，
 * 两个方向都过才说明纯 JS 实现与 Rust aes-gcm（同为标准实现）互通。
 */
import { createRequire } from 'node:module'
import crypto from 'node:crypto'
import { performance } from 'node:perf_hooks'

const require = createRequire(import.meta.url)
const WCC = require('../src-tauri/src/lan_crypto.js')

let failures = 0
function check(name, condition) {
  if (condition) console.log(`ok   ${name}`)
  else {
    failures += 1
    console.error(`FAIL ${name}`)
  }
}

// ---------- SHA-256 ----------
{
  const cases = [new Uint8Array(0), WCC.utf8('abc'), crypto.randomBytes(1000)]
  for (const [i, data] of cases.entries()) {
    const expected = crypto.createHash('sha256').update(data).digest('hex')
    check(`sha256 case ${i}`, WCC.bytesToHex(WCC.sha256(data)) === expected)
  }
}

// ---------- 密钥/路径派生 ----------
{
  const token = crypto.randomBytes(24).toString('hex')
  const device = 'phone-test-1'
  const keyExpected = crypto.createHash('sha256')
    .update('witch-clipboard-lan-e2ee-v1').update(Buffer.from(token, 'hex')).update(device).digest()
  const sidExpected = crypto.createHash('sha256')
    .update('witch-clipboard-lan-sid-v1').update(Buffer.from(token, 'hex')).update(device).digest('hex').slice(0, 24)
  check('deriveKey matches sha256(domain‖token‖device)',
    Buffer.from(WCC.deriveKey(token, device)).equals(keyExpected))
  check('deriveSid matches', WCC.deriveSid(token, device) === sidExpected)
}

function nodeEncrypt(key, nonce, aad, plain) {
  const cipher = crypto.createCipheriv('aes-256-gcm', key, nonce)
  cipher.setAAD(Buffer.from(aad))
  const ct = Buffer.concat([cipher.update(Buffer.from(plain)), cipher.final()])
  return Buffer.concat([ct, cipher.getAuthTag()])
}
function nodeDecrypt(key, nonce, aad, sealed) {
  const ct = sealed.subarray(0, sealed.length - 16)
  const tag = sealed.subarray(sealed.length - 16)
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce)
  decipher.setAAD(Buffer.from(aad))
  decipher.setAuthTag(Buffer.from(tag))
  return Buffer.concat([decipher.update(ct), decipher.final()])
}

// ---------- 固定 nonce 双向一致 ----------
{
  const key = crypto.randomBytes(32)
  const nonce = crypto.randomBytes(12)
  const aad = WCC.utf8('wcc-lan-v1:state')
  for (const size of [0, 1, 15, 16, 17, 255, 256, 1024]) {
    const plain = crypto.randomBytes(size)
    const viaNode = nodeEncrypt(key, nonce, aad, plain)
    const viaJs = Buffer.from(WCC._gcmEncryptWithNonce(key, nonce, aad, plain))
    check(`fixed-nonce byte-identical (len=${size})`, viaNode.equals(viaJs))
  }
}

// ---------- 随机参数双向互通 ----------
{
  for (let round = 0; round < 100; round++) {
    const key = crypto.randomBytes(32)
    const nonce = crypto.randomBytes(12)
    const aad = crypto.randomBytes(round % 24)
    const plain = crypto.randomBytes(Math.floor(Math.random() * 4096))

    // JS 解密 Node 密文
    const fromNode = nodeEncrypt(key, nonce, aad, plain)
    const jsPlain = WCC.decrypt(key, aad, WCC.concatBytes(nonce, fromNode))
    if (!Buffer.from(jsPlain).equals(plain)) {
      failures += 1
      console.error(`FAIL round ${round}: JS decrypt of Node ciphertext`)
    }

    // Node 解密 JS 密文（nonce 从 JS 输出里拆出来）
    const sealed = WCC.encrypt(key, aad, plain)
    const jsNonce = sealed.slice(0, 12)
    const nodePlain = nodeDecrypt(key, jsNonce, aad, sealed.slice(12))
    if (!nodePlain.equals(plain)) {
      failures += 1
      console.error(`FAIL round ${round}: Node decrypt of JS ciphertext`)
    }
  }
  console.log('ok   100 random cross-direction rounds')
}

// ---------- 篡改必须失败 / 错误 AAD 必须失败 ----------
{
  const key = crypto.randomBytes(32)
  const plain = WCC.utf8('剪贴板内容 hello')
  const aad = WCC.aad.uploadChunk('file-1', 0)
  const sealed = WCC.encrypt(key, aad, plain)
  sealed[20] ^= 1
  let threw = false
  try { WCC.decrypt(key, aad, sealed) } catch { threw = true }
  check('tampered ciphertext rejected', threw)

  threw = false
  const sealed2 = WCC.encrypt(key, aad, plain)
  try { WCC.decrypt(key, WCC.aad.uploadChunk('file-1', 1), sealed2) } catch { threw = true }
  check('wrong AAD rejected', threw)

  threw = false
  const sealed3 = WCC.encrypt(key, aad, plain)
  try { WCC.decrypt(crypto.randomBytes(32), aad, sealed3) } catch { threw = true }
  check('wrong key rejected', threw)
}

// ---------- 分块重组（模拟文件传输协议） ----------
{
  const key = crypto.randomBytes(32)
  const fileId = 'abc123'
  const file = crypto.randomBytes(600 * 1024)
  const CHUNK = 256 * 1024
  const parts = []
  for (let i = 0, off = 0; off < file.length; i++, off += CHUNK) {
    parts.push(WCC.encrypt(key, WCC.aad.fileChunk(fileId, i), file.subarray(off, off + CHUNK)))
  }
  const restored = Buffer.concat(parts.map((p, i) =>
    Buffer.from(WCC.decrypt(key, WCC.aad.fileChunk(fileId, i), p))))
  check('chunked file round-trip (600KB, 3 chunks)', restored.equals(file))
}

// ---------- 性能参考 ----------
{
  const key = crypto.randomBytes(32)
  const chunk = crypto.randomBytes(256 * 1024)
  const aad = WCC.aad.fileChunk('bench', 0)
  WCC.encrypt(key, aad, chunk) // 预热
  const t0 = performance.now()
  const sealed = WCC.encrypt(key, aad, chunk)
  const t1 = performance.now()
  WCC.decrypt(key, aad, sealed)
  const t2 = performance.now()
  console.log(`perf 256KB chunk: encrypt ${(t1 - t0).toFixed(0)}ms, decrypt ${(t2 - t1).toFixed(0)}ms (Node 桌面参考，手机端约慢 2-5 倍)`)
}

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`)
  process.exit(1)
}
console.log('\nall lan-crypto checks passed')
