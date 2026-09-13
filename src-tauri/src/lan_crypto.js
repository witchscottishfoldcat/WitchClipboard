/*
 * Witch Clipboard 局域网配对页自带密码学实现。
 *
 * 为什么不用 WebCrypto：crypto.subtle 仅在安全上下文（https 或 localhost）可用，
 * 而配对页跑在 http://192.168.x.x 这类明文局域网地址上，浏览器不提供 SubtleCrypto，
 * 只能自带纯 JS 实现。crypto.getRandomValues 在非安全上下文可用，nonce 由它生成。
 *
 * 内容：SHA-256（一次性密钥派生）+ AES-256-GCM（T 表 AES + 4-bit 表 GHASH）。
 * 正确性由 scripts/lan-crypto-test.mjs 与 Node 内置 aes-256-gcm 双向交叉验证。
 *
 * 同时兼容浏览器（挂到 window.WCC）与 Node（module.exports），不要在页面逻辑里
 * 直接展开这些内部函数——统一走 WCC.encrypt / WCC.decrypt / WCC.derive*。
 */
(function (root, factory) {
  var api = factory()
  if (typeof module === 'object' && module.exports) module.exports = api
  else root.WCC = api
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict'

  var te = new TextEncoder()
  var td = new TextDecoder()

  function utf8(s) { return te.encode(s) }
  function hexToBytes(hex) {
    var out = new Uint8Array(hex.length / 2)
    for (var i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16)
    return out
  }
  function bytesToHex(bytes) {
    var s = ''
    for (var i = 0; i < bytes.length; i++) s += bytes[i].toString(16).padStart(2, '0')
    return s
  }
  function concatBytes() {
    var total = 0
    for (var i = 0; i < arguments.length; i++) total += arguments[i].length
    var out = new Uint8Array(total), at = 0
    for (i = 0; i < arguments.length; i++) { out.set(arguments[i], at); at += arguments[i].length }
    return out
  }
  function randomBytes(n) {
    var out = new Uint8Array(n)
    // crypto.getRandomValues 在明文 http 上下文同样可用（仅 subtle 被门控）
    if (typeof crypto !== 'undefined' && crypto.getRandomValues) crypto.getRandomValues(out)
    else for (var i = 0; i < n; i++) out[i] = Math.floor(Math.random() * 256) // Node 测试兜底
    return out
  }

  // ---------------- SHA-256 ----------------
  var K256 = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ])

  function sha256(message) {
    var bitLen = message.length * 8
    var paddedLen = (((message.length + 8) >> 6) + 1) << 6
    var data = new Uint8Array(paddedLen)
    data.set(message)
    data[message.length] = 0x80
    var dv = new DataView(data.buffer)
    dv.setUint32(paddedLen - 8, Math.floor(bitLen / 0x100000000), false)
    dv.setUint32(paddedLen - 4, bitLen >>> 0, false)

    var h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a
    var h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19
    var w = new Uint32Array(64)

    for (var block = 0; block < paddedLen; block += 64) {
      for (var t = 0; t < 16; t++) w[t] = dv.getUint32(block + t * 4, false)
      for (t = 16; t < 64; t++) {
        var x = w[t - 15], y = w[t - 2]
        var s0 = ((x >>> 7) | (x << 25)) ^ ((x >>> 18) | (x << 14)) ^ (x >>> 3)
        var s1 = ((y >>> 17) | (y << 15)) ^ ((y >>> 19) | (y << 13)) ^ (y >>> 10)
        w[t] = (w[t - 16] + s0 + w[t - 7] + s1) | 0
      }
      var a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7
      for (t = 0; t < 64; t++) {
        var S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7))
        var ch = (e & f) ^ (~e & g)
        var t1 = (h + S1 + ch + K256[t] + w[t]) | 0
        var S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10))
        var maj = (a & b) ^ (a & c) ^ (b & c)
        var t2 = (S0 + maj) | 0
        h = g; g = f; f = e; e = (d + t1) | 0
        d = c; c = b; b = a; a = (t1 + t2) | 0
      }
      h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0
      h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0
    }
    var out = new Uint8Array(32)
    var odv = new DataView(out.buffer)
    var words = [h0, h1, h2, h3, h4, h5, h6, h7]
    for (var i = 0; i < 8; i++) odv.setUint32(i * 4, words[i] >>> 0, false)
    return out
  }

  // ---------------- AES-256（仅加密方向，T 表） ----------------
  var SBOX = new Uint8Array(256)
  var TE0 = new Uint32Array(256), TE1 = new Uint32Array(256)
  var TE2 = new Uint32Array(256), TE3 = new Uint32Array(256)

  ;(function buildAesTables() {
    // GF(2^8) 乘法（模 x^8+x^4+x^3+x+1）
    function mul(a, b) {
      var r = 0
      while (b) {
        if (b & 1) r ^= a
        a = (a << 1) ^ (a & 0x80 ? 0x11b : 0)
        b >>>= 1
      }
      return r & 0xff
    }
    // 乘法逆元（暴力枚举，仅建表一次性开销）
    var inv = new Uint8Array(256)
    for (var x = 1; x < 256; x++) {
      for (var y = 1; y < 256; y++) {
        if (mul(x, y) === 1) { inv[x] = y; break }
      }
    }
    function rotl8(v, n) { return ((v << n) | (v >>> (8 - n))) & 0xff }
    for (x = 0; x < 256; x++) {
      var s = inv[x] ^ rotl8(inv[x], 1) ^ rotl8(inv[x], 2) ^ rotl8(inv[x], 3) ^ rotl8(inv[x], 4) ^ 0x63
      SBOX[x] = s & 0xff
      var s2 = mul(SBOX[x], 2), s3 = mul(SBOX[x], 3)
      var word = ((s2 << 24) | (SBOX[x] << 16) | (SBOX[x] << 8) | s3) >>> 0
      TE0[x] = word
      TE1[x] = ((word << 24) | (word >>> 8)) >>> 0
      TE2[x] = ((word << 16) | (word >>> 16)) >>> 0
      TE3[x] = ((word << 8) | (word >>> 24)) >>> 0
    }
  })()

  function aesExpandKey(key) {
    // AES-256：Nk=8，Nr=14，展开成 60 个 u32
    var w = new Uint32Array(60)
    for (var i = 0; i < 8; i++) {
      w[i] = ((key[i * 4] << 24) | (key[i * 4 + 1] << 16) | (key[i * 4 + 2] << 8) | key[i * 4 + 3]) >>> 0
    }
    var rcon = 1
    for (i = 8; i < 60; i++) {
      var t = w[i - 1]
      if (i % 8 === 0) {
        t = ((SBOX[(t >>> 16) & 0xff] << 24) | (SBOX[(t >>> 8) & 0xff] << 16) |
             (SBOX[t & 0xff] << 8) | SBOX[t >>> 24]) >>> 0
        t = (t ^ (rcon << 24)) >>> 0
        rcon = (rcon << 1) ^ (rcon & 0x80 ? 0x11b : 0)
      } else if (i % 8 === 4) {
        t = ((SBOX[t >>> 24] << 24) | (SBOX[(t >>> 16) & 0xff] << 16) |
             (SBOX[(t >>> 8) & 0xff] << 8) | SBOX[t & 0xff]) >>> 0
      }
      w[i] = (w[i - 8] ^ t) >>> 0
    }
    return w
  }

  function aesEncryptBlock(w, input, inputOffset) {
    var dv = new DataView(input.buffer, input.byteOffset + (inputOffset || 0), 16)
    var s0 = (dv.getUint32(0, false) ^ w[0]) >>> 0
    var s1 = (dv.getUint32(4, false) ^ w[1]) >>> 0
    var s2 = (dv.getUint32(8, false) ^ w[2]) >>> 0
    var s3 = (dv.getUint32(12, false) ^ w[3]) >>> 0
    var t0, t1, t2, t3
    for (var round = 1; round < 14; round++) {
      var rk = round * 4
      t0 = (TE0[s0 >>> 24] ^ TE1[(s1 >>> 16) & 0xff] ^ TE2[(s2 >>> 8) & 0xff] ^ TE3[s3 & 0xff] ^ w[rk]) >>> 0
      t1 = (TE0[s1 >>> 24] ^ TE1[(s2 >>> 16) & 0xff] ^ TE2[(s3 >>> 8) & 0xff] ^ TE3[s0 & 0xff] ^ w[rk + 1]) >>> 0
      t2 = (TE0[s2 >>> 24] ^ TE1[(s3 >>> 16) & 0xff] ^ TE2[(s0 >>> 8) & 0xff] ^ TE3[s1 & 0xff] ^ w[rk + 2]) >>> 0
      t3 = (TE0[s3 >>> 24] ^ TE1[(s0 >>> 16) & 0xff] ^ TE2[(s1 >>> 8) & 0xff] ^ TE3[s2 & 0xff] ^ w[rk + 3]) >>> 0
      s0 = t0; s1 = t1; s2 = t2; s3 = t3
    }
    // 末轮无 MixColumns，且行移位按标准展开
    var out = new Uint8Array(16)
    out[0] = SBOX[s0 >>> 24]; out[1] = SBOX[(s1 >>> 16) & 0xff]; out[2] = SBOX[(s2 >>> 8) & 0xff]; out[3] = SBOX[s3 & 0xff]
    out[4] = SBOX[s1 >>> 24]; out[5] = SBOX[(s2 >>> 16) & 0xff]; out[6] = SBOX[(s3 >>> 8) & 0xff]; out[7] = SBOX[s0 & 0xff]
    out[8] = SBOX[s2 >>> 24]; out[9] = SBOX[(s3 >>> 16) & 0xff]; out[10] = SBOX[(s0 >>> 8) & 0xff]; out[11] = SBOX[s1 & 0xff]
    out[12] = SBOX[s3 >>> 24]; out[13] = SBOX[(s0 >>> 16) & 0xff]; out[14] = SBOX[(s1 >>> 8) & 0xff]; out[15] = SBOX[s2 & 0xff]
    var odv = new DataView(out.buffer)
    odv.setUint32(0, (odv.getUint32(0, false) ^ w[56]) >>> 0, false)
    odv.setUint32(4, (odv.getUint32(4, false) ^ w[57]) >>> 0, false)
    odv.setUint32(8, (odv.getUint32(8, false) ^ w[58]) >>> 0, false)
    odv.setUint32(12, (odv.getUint32(12, false) ^ w[59]) >>> 0, false)
    return out
  }

  // ---------------- GHASH（4-bit 表，BigInt 表示 128 位域元素） ----------------
  var R_POLY = 0xe1n << 120n
  var MASK128 = (1n << 128n) - 1n

  function bytesToBigIntBE(bytes) {
    var v = 0n
    for (var i = 0; i < bytes.length; i++) v = (v << 8n) | BigInt(bytes[i])
    return v
  }
  function bigIntToBytesBE(v, length) {
    var out = new Uint8Array(length)
    for (var i = length - 1; i >= 0; i--) { out[i] = Number(v & 0xffn); v >>= 8n }
    return out
  }

  function makeGhashTable(hBytes) {
    var h = bytesToBigIntBE(hBytes)
    function shr1(v) { return (v >> 1n) ^ ((v & 1n) ? R_POLY : 0n) }
    // RED[f] = shr4(f)：右移 4 位时掉出的低 4 位需要折叠回高端
    var red = new Array(16)
    for (var f = 0; f < 16; f++) {
      var v = BigInt(f)
      v = shr1(shr1(shr1(shr1(v))))
      red[f] = v
    }
    // P[j]：j 的 4 个比特（最高位对应域元素的 x^0 系数）分别对应 shr1^b(H)
    var base = [h, shr1(h), shr1(shr1(h)), shr1(shr1(shr1(h)))]
    var p = new Array(16)
    for (var j = 0; j < 16; j++) {
      p[j] = ((j & 8) ? base[0] : 0n) ^ ((j & 4) ? base[1] : 0n) ^
             ((j & 2) ? base[2] : 0n) ^ ((j & 1) ? base[3] : 0n)
    }
    return { p: p, red: red }
  }

  function ghashMul(tab, z) {
    // z · H：扫描 z 的 32 个半字节（MSB 优先），Horner 顺序自低位组向高位组累积
    var acc = 0n
    for (var k = 31; k >= 0; k--) {
      var nib = Number((z >> BigInt(124 - 4 * k)) & 15n)
      acc = tab.p[nib] ^ ((acc >> 4n) ^ tab.red[Number(acc & 15n)])
    }
    return acc & MASK128
  }

  function ghash(tab, aad, ciphertext) {
    var z = 0n
    function absorb(bytes) {
      for (var off = 0; off < bytes.length; off += 16) {
        var block = bytes.slice(off, Math.min(off + 16, bytes.length))
        if (block.length < 16) {
          var padded = new Uint8Array(16)
          padded.set(block)
          block = padded
        }
        z = ghashMul(tab, z ^ bytesToBigIntBE(block))
      }
    }
    absorb(aad)
    absorb(ciphertext)
    var lenBlock = (BigInt(aad.length) * 8n << 64n) | (BigInt(ciphertext.length) * 8n)
    z = ghashMul(tab, z ^ lenBlock)
    return bigIntToBytesBE(z, 16)
  }

  // ---------------- AES-256-GCM ----------------
  function inc32(counterBlock) {
    var out = counterBlock.slice()
    for (var i = 15; i >= 12; i--) {
      out[i] = (out[i] + 1) & 0xff
      if (out[i] !== 0) break
    }
    return out
  }

  function gcmCrypt(w, j0, data) {
    var out = new Uint8Array(data.length)
    var counter = j0
    for (var off = 0; off < data.length; off += 16) {
      counter = inc32(counter)
      var stream = aesEncryptBlock(w, counter, 0)
      var n = Math.min(16, data.length - off)
      for (var i = 0; i < n; i++) out[off + i] = data[off + i] ^ stream[i]
    }
    return out
  }

  /** 返回 nonce(12) || ciphertext || tag(16)，与 Rust 端 lan_seal 布局一致 */
  function gcmEncrypt(key, aad, plain) {
    var w = aesExpandKey(key)
    var nonce = randomBytes(12)
    return concatBytes(nonce, gcmEncryptWithNonce(w, nonce, aad, plain))
  }

  function gcmEncryptWithNonce(w, nonce, aad, plain) {
    var j0 = concatBytes(nonce, new Uint8Array([0, 0, 0, 1]))
    var ciphertext = gcmCrypt(w, j0, plain)
    var hBytes = aesEncryptBlock(w, new Uint8Array(16), 0)
    var tab = makeGhashTable(hBytes)
    var s = ghash(tab, aad, ciphertext)
    var ej0 = aesEncryptBlock(w, j0, 0)
    var tag = new Uint8Array(16)
    for (var i = 0; i < 16; i++) tag[i] = s[i] ^ ej0[i]
    return concatBytes(ciphertext, tag)
  }

  /** 校验失败抛异常；成功返回明文 Uint8Array */
  function gcmDecrypt(key, aad, sealed) {
    if (sealed.length < 12 + 16) throw new Error('sealed payload too short')
    var nonce = sealed.slice(0, 12)
    var ciphertext = sealed.slice(12, sealed.length - 16)
    var expectedTag = sealed.slice(sealed.length - 16)
    var w = aesExpandKey(key)
    var j0 = concatBytes(nonce, new Uint8Array([0, 0, 0, 1]))
    var hBytes = aesEncryptBlock(w, new Uint8Array(16), 0)
    var tab = makeGhashTable(hBytes)
    var s = ghash(tab, aad, ciphertext)
    var ej0 = aesEncryptBlock(w, j0, 0)
    var diff = 0
    for (var i = 0; i < 16; i++) diff |= s[i] ^ ej0[i] ^ expectedTag[i]
    if (diff !== 0) throw new Error('GCM authentication failed')
    return gcmCrypt(w, j0, ciphertext)
  }

  // ---------------- 与 Rust 端共享的派生参数 ----------------
  var DOMAIN_KDF = 'witch-clipboard-lan-e2ee-v1'
  var DOMAIN_SID = 'witch-clipboard-lan-sid-v1'

  /** 会话密钥：SHA-256(域 ‖ token 字节 ‖ 设备 id)。token 只随配对页出现一次，之后不上线 */
  function deriveKey(tokenHex, deviceId) {
    return sha256(concatBytes(utf8(DOMAIN_KDF), hexToBytes(tokenHex), utf8(deviceId)))
  }

  /** 会话路径 id：派生自同样材料，代替 token 出现在后续 API 路径里 */
  function deriveSid(tokenHex, deviceId) {
    return bytesToHex(sha256(concatBytes(utf8(DOMAIN_SID), hexToBytes(tokenHex), utf8(deviceId)))).slice(0, 24)
  }

  // AAD 约定：与 Rust 端逐字节一致
  var aad = {
    state: utf8('wcc-lan-v1:state'),
    image: utf8('wcc-lan-v1:image'),
    send: utf8('wcc-lan-v1:send'),
    uploadInit: utf8('wcc-lan-v1:upload-init'),
    uploadChunk: function (id, offset) { return utf8('wcc-lan-v1:upload-chunk:' + id + ':' + offset) },
    fileChunk: function (id, index) { return utf8('wcc-lan-v1:file-chunk:' + id + ':' + index) },
  }

  return {
    sha256: sha256,
    deriveKey: deriveKey,
    deriveSid: deriveSid,
    aad: aad,
    encrypt: gcmEncrypt,
    decrypt: gcmDecrypt,
    // 测试与页面都用得到的编解码助手
    utf8: utf8,
    hexToBytes: hexToBytes,
    bytesToHex: bytesToHex,
    concatBytes: concatBytes,
    decodeUtf8: function (bytes) { return td.decode(bytes) },
    // 仅供交叉验证脚本使用
    _gcmEncryptWithNonce: function (key, nonce, aad, plain) {
      return gcmEncryptWithNonce(aesExpandKey(key), nonce, aad, plain)
    },
  }
})
