import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import { getToken, logoutAndRedirect, setAuthProvider } from './api'
import { useEffect, useRef, useState } from 'react'
import { api } from './api'
import { Icon } from './components/Icons'
import { useConfirm } from './components/Confirm'
import { APP_VERSION } from './utils/version'

const SIDEBAR_COLLAPSED_KEY = 'pertisk_kos_sidebar_collapsed'

const NAV = [
  { to: '/', label: 'Dashboard', icon: 'dashboard', end: true },
  { to: '/clusters', label: 'Clusters', icon: 'clusters' },
  { to: '/machines', label: 'Machines', icon: 'machines' },
  { to: '/templates', label: 'Templates', icon: 'templates' },
  { to: '/providers', label: 'Providers', icon: 'providers' },
  { to: '/audit', label: 'Audit', icon: 'audit' },
  { to: '/settings', label: 'Settings', icon: 'settings' },
]

function getStoredCollapsed() {
  return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === 'true'
}

function resolveTitle(pathname) {
  const match = NAV.filter((n) =>
    n.end ? pathname === n.to : pathname === n.to || pathname.startsWith(`${n.to}/`),
  ).sort((a, b) => b.to.length - a.to.length)[0]
  return match?.label ?? 'Cluster management'
}

export default function Layout() {
  const nav = useNavigate()
  const location = useLocation()
  const confirm = useConfirm()
  const [user, setUser] = useState(null)
  const [theme, setTheme] = useState(() => localStorage.getItem('theme') || 'dark')
  const [showUserMenu, setShowUserMenu] = useState(false)
  const [mobileOpen, setMobileOpen] = useState(false)
  const [collapsed, setCollapsed] = useState(getStoredCollapsed)
  const userMenuRef = useRef(null)
  const title = resolveTitle(location.pathname)

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme)
    localStorage.setItem('theme', theme)
  }, [theme])

  useEffect(() => {
    if (!getToken()) {
      nav('/login')
      return
    }
    api('/auth/me')
      .then((u) => {
        if (u?.provider) setAuthProvider(u.provider)
        setUser(u)
      })
      .catch(() => nav('/login'))
  }, [nav])

  useEffect(() => {
    setMobileOpen(false)
    setShowUserMenu(false)
  }, [location.pathname])

  useEffect(() => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed))
  }, [collapsed])

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

  useEffect(() => {
    if (!mobileOpen) return
    function onKey(e) {
      if (e.key === 'Escape') setMobileOpen(false)
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [mobileOpen])

  async function logout() {
    setShowUserMenu(false)
    const ok = await confirm({
      title: 'Sign out',
      message:
        user?.provider === 'auth0'
          ? 'End your session and clear Auth0 SSO on this device?'
          : 'End your session on this device?',
      confirmLabel: 'Sign out',
      tone: 'primary',
    })
    if (!ok) return
    logoutAndRedirect(user?.provider || 'local')
  }

  const initial = user?.username ? user.username.charAt(0).toUpperCase() : 'U'

  return (
    <div className="shell">
      <div
        className={`sidebar-backdrop${mobileOpen ? ' open' : ''}`}
        aria-hidden={!mobileOpen}
        onClick={() => setMobileOpen(false)}
      />

      <aside
        id="app-sidebar"
        className={`sidebar${mobileOpen ? ' open' : ''}${collapsed ? ' collapsed' : ''}`}
      >
        <div className="sidebar-header">
          <div className="brand">
            <span className="brand-mark">P</span>
            <div className="brand-text">
              <span>
                Pertisk <span className="accent">KOS</span>
              </span>
              <span className="brand-version">v{APP_VERSION}</span>
            </div>
          </div>
          <button
            type="button"
            className={`sidebar-collapse-btn${!collapsed ? ' anchor-right' : ''}`}
            onClick={() => setCollapsed((v) => !v)}
            title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
            aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          >
            <Icon name={collapsed ? 'chevrons-right' : 'chevrons-left'} size={16} />
          </button>
        </div>

        <nav className="nav" aria-label="Primary">
          {NAV.map(({ to, label, icon, end }) => (
            <NavLink
              key={to}
              to={to}
              end={end}
              title={collapsed ? label : undefined}
              onClick={() => setMobileOpen(false)}
              className={({ isActive }) => (isActive ? 'active' : undefined)}
            >
              <Icon name={icon} size={18} />
              <span className="nav-label">{label}</span>
            </NavLink>
          ))}
        </nav>
      </aside>

      <div className={`main${mobileOpen ? ' sidebar-open' : ''}`}>
        <header className="topbar">
          <div className="topbar-left">
            <button
              type="button"
              className="secondary btn-icon topbar-menu-btn"
              aria-controls="app-sidebar"
              aria-expanded={mobileOpen}
              aria-label={mobileOpen ? 'Close menu' : 'Open menu'}
              onClick={() => setMobileOpen((v) => !v)}
            >
              <Icon name={mobileOpen ? 'x' : 'menu'} size={18} />
            </button>
            <h1 className="topbar-title">{title}</h1>
          </div>
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
                    <Icon name="logout" size={14} /> Logout
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
