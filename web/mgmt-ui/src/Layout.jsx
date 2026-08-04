import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import { clearToken, getToken } from './api'
import { useEffect, useRef, useState } from 'react'
import { api } from './api'
import { Icon } from './components/Icons'
import { useConfirm } from './components/Confirm'
import { APP_VERSION } from './utils/version'

export default function Layout() {
  const nav = useNavigate()
  const confirm = useConfirm()
  const [user, setUser] = useState(null)
  const [theme, setTheme] = useState(() => localStorage.getItem('theme') || 'dark')
  const [showUserMenu, setShowUserMenu] = useState(false)
  const userMenuRef = useRef(null)

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme)
    localStorage.setItem('theme', theme)
  }, [theme])

  useEffect(() => {
    if (!getToken()) {
      nav('/login')
      return
    }
    api('/auth/me').then(setUser).catch(() => nav('/login'))
  }, [nav])

  useEffect(() => {
    if (!showUserMenu) return
    function onPointerDown(e) {
      if (userMenuRef.current && !userMenuRef.current.contains(e.target)) {
        setShowUserMenu(false)
      }
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  }, [showUserMenu])

  async function logout() {
    setShowUserMenu(false)
    const ok = await confirm({
      title: 'Sign out',
      message: 'End your session on this device?',
      confirmLabel: 'Sign out',
      tone: 'primary',
    })
    if (!ok) return
    clearToken()
    nav('/login')
  }

  const initial = user?.username ? user.username.charAt(0).toUpperCase() : 'U'

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">P</span>
          <div className="brand-text">
            <span>Pertisk <span className="accent">KOS</span></span>
            <span className="brand-version">v{APP_VERSION}</span>
          </div>
        </div>
        <nav className="nav">
          <NavLink to="/" end>
            <Icon name="dashboard" size={18} /> Dashboard
          </NavLink>
          <NavLink to="/clusters">
            <Icon name="clusters" size={18} /> Clusters
          </NavLink>
          <NavLink to="/providers">
            <Icon name="providers" size={18} /> Providers
          </NavLink>
          <NavLink to="/settings">
            <Icon name="settings" size={18} /> Settings
          </NavLink>
        </nav>
      </aside>
      <div className="main">
        <header className="topbar">
          <div className="muted">Cluster management</div>
          <div className="row-actions">
            <button
              type="button"
              className="secondary btn-icon"
              onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
              title="Toggle theme"
            >
              <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={16} />
            </button>
            <div className="user-menu" ref={userMenuRef}>
              <button
                type="button"
                className={`user-menu-trigger${showUserMenu ? ' open' : ''}`}
                onClick={() => setShowUserMenu((v) => !v)}
                aria-haspopup="menu"
                aria-expanded={showUserMenu}
              >
                <span className="user-avatar">{initial}</span>
                <span className="user-name">{user?.username || 'User'}</span>
                <Icon name="chevron-down" size={14} className="user-chevron" />
              </button>
              {showUserMenu && (
                <div className="user-menu-dropdown" role="menu">
                  {user?.role && <div className="user-menu-meta">{user.role}</div>}
                  <button type="button" role="menuitem" onClick={logout}>
                    Logout
                  </button>
                </div>
              )}
            </div>
          </div>
        </header>
        <div className="content">
          <Outlet />
        </div>
      </div>
    </div>
  )
}
