import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import { getToken, logoutAndRedirect, setAuthProvider } from './api'
import { useEffect, useMemo, useRef, useState } from 'react'
import { api } from './api'
import { Icon } from './components/Icons'
import BrandLogo from './components/BrandLogo'
import ThemeToggle from './components/ThemeToggle'
import BottomShell from './components/BottomShell'
import { useConfirm } from './components/Confirm'
import { APP_VERSION } from './utils/version'
import { useShellDock } from './shell/ShellDockContext'

const SIDEBAR_COLLAPSED_KEY = 'pertisk_kos_sidebar_collapsed'

const NAV_SECTIONS = [
  {
    label: 'Fleet',
    items: [
      { to: '/', label: 'Overview', icon: 'dashboard', end: true },
      { to: '/clusters', label: 'Clusters', icon: 'clusters' },
      { to: '/machines', label: 'Machines', icon: 'machines' },
      { to: '/providers', label: 'Providers', icon: 'providers' },
    ],
  },
  {
    label: 'Node OS',
    items: [
      { to: '/images', label: 'Images', icon: 'disk' },
      { to: '/os-packages', label: 'OS packages', icon: 'packages' },
      { to: '/templates', label: 'Templates', icon: 'templates' },
    ],
  },
  {
    label: 'System',
    items: [
      { to: '/users', label: 'Users', icon: 'users', adminOnly: true },
      { to: '/audit', label: 'Audit log', icon: 'audit' },
      { to: '/settings', label: 'Settings', icon: 'settings' },
    ],
  },
]

const SECTION_TITLES = [
  { match: /^\/clusters\/[^/]+/, title: 'Cluster' },
  { match: /^\/clusters/, title: 'Clusters' },
  { match: /^\/machines\/[^/]+/, title: 'Machine' },
  { match: /^\/machines/, title: 'Machines' },
  { match: /^\/providers\/[^/]+/, title: 'Provider' },
  { match: /^\/providers/, title: 'Providers' },
  { match: /^\/images/, title: 'Images' },
  { match: /^\/os-packages/, title: 'OS packages' },
  { match: /^\/templates/, title: 'Templates' },
  { match: /^\/users/, title: 'Users' },
  { match: /^\/audit/, title: 'Audit log' },
  { match: /^\/settings/, title: 'Settings' },
  { match: /^\/$/, title: 'Overview' },
]

function getStoredCollapsed() {
  return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === 'true'
}

function sectionTitle(pathname) {
  for (const entry of SECTION_TITLES) {
    if (entry.match.test(pathname)) return entry.title
  }
  return 'Overview'
}

export default function Layout() {
  const nav = useNavigate()
  const location = useLocation()
  const confirm = useConfirm()
  const shellDock = useShellDock()
  const [user, setUser] = useState(null)
  const [showUserMenu, setShowUserMenu] = useState(false)
  const [mobileOpen, setMobileOpen] = useState(false)
  const [collapsed, setCollapsed] = useState(getStoredCollapsed)
  const [search, setSearch] = useState('')
  const userMenuRef = useRef(null)

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
          ? 'End your Pertisk session and clear Auth0 for this app? Your Google or other accounts stay signed in.'
          : 'End your session on this device?',
      confirmLabel: 'Sign out',
      tone: 'primary',
    })
    if (!ok) return
    logoutAndRedirect(user?.provider || 'local')
  }

  function onSearch(e) {
    e.preventDefault()
    const q = search.trim()
    if (!q) {
      nav('/machines')
      return
    }
    nav(`/machines?q=${encodeURIComponent(q)}`)
  }

  const initial = user?.username ? user.username.slice(0, 2).toUpperCase() : 'AD'
  const title = useMemo(() => sectionTitle(location.pathname), [location.pathname])
  const sections = NAV_SECTIONS.map((section) => ({
    ...section,
    items: section.items.filter((n) => !n.adminOnly || user?.role === 'admin'),
  })).filter((section) => section.items.length > 0)

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
          <NavLink to="/" className="brand" onClick={() => setMobileOpen(false)}>
            <span className="brand-mark" aria-hidden>
              <BrandLogo size={28} />
            </span>
            <span className="brand-text">
              <span className="brand-name">Pertisk KOS</span>
              <span className="brand-version">v{APP_VERSION}</span>
            </span>
          </NavLink>
          <button
            type="button"
            className="sidebar-close-btn"
            onClick={() => setMobileOpen(false)}
            aria-label="Close navigation"
          >
            <Icon name="x" size={18} />
          </button>
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
          {sections.map((section) => (
            <div className="nav-group" key={section.label}>
              <p className="nav-heading">{section.label}</p>
              <ul>
                {section.items.map(({ to, label, icon, end }) => (
                  <li key={to}>
                    <NavLink
                      to={to}
                      end={end}
                      title={collapsed ? label : undefined}
                      onClick={() => setMobileOpen(false)}
                      className={({ isActive }) => (isActive ? 'active' : undefined)}
                    >
                      <Icon name={icon} size={16} />
                      <span className="nav-label">{label}</span>
                    </NavLink>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </nav>
      </aside>

      <div className={`main${mobileOpen ? ' sidebar-open' : ''}`}>
        <header className="topbar">
          <div className="topbar-left">
            <button
              type="button"
              className="theme-toggle topbar-menu-btn"
              aria-controls="app-sidebar"
              aria-expanded={mobileOpen}
              aria-label={mobileOpen ? 'Close menu' : 'Open menu'}
              onClick={() => setMobileOpen((v) => !v)}
            >
              <Icon name={mobileOpen ? 'x' : 'menu'} size={16} />
            </button>
            <div className="topbar-section">{title}</div>
            <form className="topbar-search" onSubmit={onSearch} role="search">
              <Icon name="search" size={14} />
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search clusters, nodes, node IDs…"
                aria-label="Search"
              />
            </form>
          </div>
          <div className="topbar-actions">
            <button
              type="button"
              className="theme-toggle"
              title="Management shell"
              aria-label="Open management shell"
              onClick={() => shellDock.openMgmtShell()}
            >
              <Icon name="terminal" size={16} />
            </button>
            <ThemeToggle />
            <button
              type="button"
              className="btn topbar-cta"
              onClick={() => nav('/clusters?new=1')}
            >
              <Icon name="plus" size={14} />
              New cluster
            </button>
            <div className="topbar-user-chip user-menu" ref={userMenuRef}>
              <button
                type="button"
                className={`user-menu-trigger${showUserMenu ? ' open' : ''}`}
                onClick={() => setShowUserMenu((v) => !v)}
                aria-haspopup="menu"
                aria-expanded={showUserMenu}
                aria-label={user?.username ? `Signed in as ${user.username}` : 'Account menu'}
              >
                <span className="user-avatar">{initial}</span>
              </button>
              {showUserMenu && (
                <div className="user-menu-dropdown" role="menu">
                  {user?.username && <div className="user-menu-meta">{user.username}</div>}
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
        <BottomShell />
      </div>
    </div>
  )
}
