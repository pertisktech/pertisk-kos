import { Link } from 'react-router-dom'
import { Icon } from './Icons'
import ThemeToggle from './ThemeToggle'
import { APP_VERSION } from '../utils/version'

export default function AuthLayout({ title, subtitle, children }) {
  return (
    <div className="auth-shell">
      <aside className="auth-brand-panel">
        <Link to="/login" className="auth-brand-link">
          <span className="brand-mark" aria-hidden>
            <Icon name="clusters" size={16} />
          </span>
          <span className="auth-brand-name">Pertisk KOS</span>
        </Link>
        <div className="auth-hero">
          <h1>The control plane for immutable Kubernetes node operating systems.</h1>
          <p>
            Provision clusters, publish signed OS images, and reconcile hypervisors across your
            entire fleet — from one place.
          </p>
        </div>
        <p className="auth-version">Pertisk KOS v{APP_VERSION}</p>
      </aside>

      <div className="auth-main">
        <div className="auth-theme">
          <ThemeToggle />
        </div>
        <div className="auth-form">
          <div className="auth-mobile-brand">
            <span className="brand-mark" aria-hidden>
              <Icon name="clusters" size={16} />
            </span>
            <span className="auth-brand-name">Pertisk KOS</span>
          </div>
          <div className="auth-heading">
            <h2>{title}</h2>
            {subtitle ? <p>{subtitle}</p> : null}
          </div>
          {children}
        </div>
      </div>
    </div>
  )
}
