/** Pertisk KOS mark from the redesign favicon / logo. */
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
