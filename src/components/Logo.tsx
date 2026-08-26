// 界面与系统图标共用同一光栅母版；界面载入 256px 派生图。
import logoUrl from '@res/icon-256.png'

interface Props {
  className?: string
  title?: string
}

export function Logo({ className = '', title }: Props) {
  return (
    <img
      src={logoUrl}
      className={`block object-contain ${className}`}
      title={title}
      alt=""
      aria-label="Witch Clipboard"
      role="img"
      draggable={false}
    />
  )
}
