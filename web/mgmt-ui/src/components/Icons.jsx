/** Inline SVG icons (24px viewBox). */
export function Icon({ name, size = 18, className = '' }) {
  const props = {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 1.75,
    strokeLinecap: 'round',
    strokeLinejoin: 'round',
    className: `icon ${className}`,
    'aria-hidden': true,
  }
  switch (name) {
    case 'dashboard':
      return (
        <svg {...props}>
          <rect x="3" y="3" width="7" height="9" rx="1.5" />
          <rect x="14" y="3" width="7" height="5" rx="1.5" />
          <rect x="14" y="12" width="7" height="9" rx="1.5" />
          <rect x="3" y="16" width="7" height="5" rx="1.5" />
        </svg>
      )
    case 'clusters':
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="3" />
          <circle cx="5" cy="7" r="2" />
          <circle cx="19" cy="7" r="2" />
          <circle cx="5" cy="17" r="2" />
          <circle cx="19" cy="17" r="2" />
          <path d="M7 8.5 10 10.5M14 10.5 17 8.5M7 15.5 10 13.5M14 13.5 17 15.5" />
        </svg>
      )
    case 'providers':
      return (
        <svg {...props}>
          <rect x="3" y="4" width="18" height="14" rx="2" />
          <path d="M3 9h18M8 14h2M13 14h3" />
        </svg>
      )
    case 'machines':
      return (
        <svg {...props}>
          <rect x="4" y="4" width="16" height="12" rx="1.5" />
          <path d="M8 20h8M12 16v4" />
        </svg>
      )
    case 'templates':
      return (
        <svg {...props}>
          <path d="M7 3h7l5 5v13a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" />
          <path d="M14 3v5h5M9 13h6M9 17h4" />
        </svg>
      )
    case 'audit':
      return (
        <svg {...props}>
          <path d="M8 4h8a2 2 0 0 1 2 2v14l-6-3-6 3V6a2 2 0 0 1 2-2z" />
          <path d="M10 10h4M10 14h2" />
        </svg>
      )
    case 'settings':
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="3" />
          <path d="M12 3v2M12 19v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M3 12h2M19 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
        </svg>
      )
    case 'plus':
      return (
        <svg {...props}>
          <path d="M12 5v14M5 12h14" />
        </svg>
      )
    case 'trash':
      return (
        <svg {...props}>
          <path d="M4 7h16M9 7V5h6v2M8 7l1 12h6l1-12" />
        </svg>
      )
    case 'edit':
      return (
        <svg {...props}>
          <path d="M4 20h4l10-10-4-4L4 16v4zM13 7l4 4" />
        </svg>
      )
    case 'check':
      return (
        <svg {...props}>
          <path d="M5 12l5 5L20 7" />
        </svg>
      )
    case 'x':
      return (
        <svg {...props}>
          <path d="M6 6l12 12M18 6 6 18" />
        </svg>
      )
    case 'download':
      return (
        <svg {...props}>
          <path d="M12 4v12M8 12l4 4 4-4M5 20h14" />
        </svg>
      )
    case 'external':
      return (
        <svg {...props}>
          <path d="M14 4h6v6M10 14 20 4M5 9v10a1 1 0 0 0 1 1h10" />
        </svg>
      )
    case 'sun':
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="4" />
          <path d="M12 2v2M12 20v2M4 12H2M22 12h-2M5 5l1.5 1.5M17.5 17.5 19 19M19 5l-1.5 1.5M5 19l1.5-1.5" />
        </svg>
      )
    case 'moon':
      return (
        <svg {...props}>
          <path d="M20 14.5A8 8 0 0 1 9.5 4 7 7 0 1 0 20 14.5z" />
        </svg>
      )
    case 'logout':
      return (
        <svg {...props}>
          <path d="M10 7V5a2 2 0 0 1 2-2h7v18h-7a2 2 0 0 1-2-2v-2M15 12H3M6 9l-3 3 3 3" />
        </svg>
      )
    case 'play':
      return (
        <svg {...props}>
          <path d="M8 5v14l12-7z" fill="currentColor" stroke="none" />
        </svg>
      )
    case 'cpu':
      return (
        <svg {...props}>
          <rect x="6" y="6" width="12" height="12" rx="2" />
          <path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4" />
        </svg>
      )
    case 'memory':
      return (
        <svg {...props}>
          <rect x="3" y="7" width="18" height="10" rx="2" />
          <path d="M7 7v10M11 7v10M15 7v10M7 4v3M12 4v3M17 4v3M7 17v3M12 17v3M17 17v3" />
        </svg>
      )
    case 'disk':
      return (
        <svg {...props}>
          <ellipse cx="12" cy="6" rx="8" ry="3" />
          <path d="M4 6v6c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6" />
        </svg>
      )
    case 'worker':
      return (
        <svg {...props}>
          <rect x="3" y="8" width="18" height="10" rx="2" />
          <path d="M7 8V6a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v2M8 13h2M14 13h2" />
        </svg>
      )
    case 'network':
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="8" />
          <path d="M3 12h18M12 4a14 14 0 0 1 0 16M12 4a14 14 0 0 0 0 16" />
        </svg>
      )
    case 'alert':
      return (
        <svg {...props}>
          <path d="M12 3 2 20h20L12 3z" />
          <path d="M12 10v4M12 17h.01" />
        </svg>
      )
    case 'back':
      return (
        <svg {...props}>
          <path d="M15 6 9 12l6 6" />
        </svg>
      )
    case 'reboot':
      return (
        <svg {...props}>
          <path d="M4 4v5h5" />
          <path d="M20 20v-5h-5" />
          <path d="M5.6 9A7 7 0 0 1 19 7.5M18.4 15A7 7 0 0 1 5 16.5" />
        </svg>
      )
    case 'chevron-down':
      return (
        <svg {...props}>
          <path d="M6 9l6 6 6-6" />
        </svg>
      )
    case 'menu':
      return (
        <svg {...props}>
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      )
    case 'chevrons-left':
      return (
        <svg {...props}>
          <path d="M11 6 5 12l6 6M19 6l-6 6 6 6" />
        </svg>
      )
    case 'chevrons-right':
      return (
        <svg {...props}>
          <path d="M13 6l6 6-6 6M5 6l6 6-6 6" />
        </svg>
      )
    default:
      return null
  }
}

export function Btn({ icon, children, variant = 'primary', className = '', ...rest }) {
  const v = variant === 'primary' ? '' : variant
  return (
    <button type="button" className={`btn-icon ${v} ${className}`.trim()} {...rest}>
      {icon && <Icon name={icon} size={16} />}
      {children && <span>{children}</span>}
    </button>
  )
}
