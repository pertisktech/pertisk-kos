import BrandLogo from './BrandLogo'
import ThemeToggle from './ThemeToggle'
import { APP_VERSION } from '../utils/version'

export default function AuthLayout({ title, subtitle, children }) {
  return (
    <div className="auth-shell">
      <div className="auth-theme">
        <ThemeToggle />
      </div>
      <div className="auth-main">
        <div className="auth-form">
          <div className="auth-mobile-brand">
            <span className="brand-mark" aria-hidden>
              <BrandLogo size={28} />
            </span>
            <div>
              <div className="auth-brand-name">
                pertisk<span className="brand-slash">/</span>kos
              </div>
              <div className="auth-brand-sub">Kubernetes operating system</div>
            </div>
          </div>
          <div className="auth-heading">
            <h2>{title || 'Sign in to KOS'}</h2>
            {subtitle ? <p>{subtitle}</p> : null}
          </div>
          {children}
          <div className="auth-footer">Pertisk KOS v{APP_VERSION} · Secure control plane</div>
        </div>
      </div>
    </div>
  )
}
