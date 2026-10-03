/** Pertisk KOS mark: violet tile + radio tower, matching designs/pertisk-kos. */
export default function BrandLogo({ className = 'brand-logo', size = 28, title = 'Pertisk KOS' }) {
  return (
    <img
      className={className}
      src="/logo.svg"
      width={size}
      height={size}
      alt=""
      title={title}
      decoding="async"
    />
  )
}
