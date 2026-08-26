/**
 * 图标生成：高保真 PNG 是设计母版，各尺寸都由它统一缩放生成。
 *
 * 用 Electron（Chromium）缩放 PNG——它本来就在依赖里，不用再引 sharp/resvg 这类
 * 需要编译的依赖。画到 canvas 再取 toDataURL，拿到的是真正带 alpha 的位图，
 * 不受窗口透明度设置影响。
 *
 * 运行：npm run icons
 */
const { app, BrowserWindow, nativeImage } = require('electron')
const { readFileSync, writeFileSync, mkdirSync } = require('node:fs')
const { join } = require('node:path')

const RES = join(__dirname, '..', 'resources')

/** [源图, 输出 PNG, 边长] */
const JOBS = [
  ['logo-rendered.png', 'icon.png', 512],
  ['logo-rendered.png', 'icon-256.png', 256],
  ['logo-rendered.png', 'tray.png', 32],
  ['logo-rendered.png', 'tray@2x.png', 64],
]

// 新母版的帽子、底板和边框已经贴合画布，额外裁边会破坏完整轮廓。
const OUTER_CROP_RATIO = 0

async function rasterize(win, sourcePath, size) {
  const source = readFileSync(join(RES, sourcePath))
  const mime = sourcePath.endsWith('.svg') ? 'image/svg+xml' : 'image/png'
  const src = `data:${mime};base64,${source.toString('base64')}`

  const dataUrl = await win.webContents.executeJavaScript(
    `(async () => {
      const img = new Image()
      img.src = ${JSON.stringify(src)}
      await img.decode()
      const canvas = document.createElement('canvas')
      canvas.width = ${size}
      canvas.height = ${size}
      const ctx = canvas.getContext('2d')
      ctx.imageSmoothingEnabled = true
      ctx.imageSmoothingQuality = 'high'
      const cropX = img.naturalWidth * ${OUTER_CROP_RATIO}
      const cropY = img.naturalHeight * ${OUTER_CROP_RATIO}
      ctx.drawImage(
        img,
        cropX,
        cropY,
        img.naturalWidth - cropX * 2,
        img.naturalHeight - cropY * 2,
        0,
        0,
        ${size},
        ${size},
      )
      return canvas.toDataURL('image/png')
    })()`,
    true,
  )

  const png = nativeImage.createFromDataURL(dataUrl).toPNG()
  if (png.byteLength === 0) throw new Error(`${svgPath} 光栅化结果为空`)
  return png
}

app
  .whenReady()
  .then(async () => {
    const win = new BrowserWindow({ width: 64, height: 64, show: false })
    await win.loadURL('data:text/html,<meta charset="utf-8"><body></body>')

    mkdirSync(RES, { recursive: true })
    let failed = 0

    for (const [sourcePath, out, size] of JOBS) {
      try {
        const png = await rasterize(win, sourcePath, size)
        writeFileSync(join(RES, out), png)
        console.log(`✓ resources/${out}  ${size}x${size}  ${(png.byteLength / 1024).toFixed(1)} KB`)
      } catch (err) {
        failed++
        console.error(`✗ resources/${out}  ${err.message}`)
      }
    }

    win.destroy()
    app.exit(failed === 0 ? 0 : 1)
  })
  .catch((err) => {
    console.error('图标生成失败：', err)
    app.exit(1)
  })
