import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import { clearToken, getToken } from './api'
import { useEffect, useState } from 'react'
import { api } from './api'

export default function Layout() {
  const nav = useNavigate()
  const [user, setUser] = useState(null)
  const [theme, setTheme] = useState(() => localStorage.getItem('theme') || 'dark')

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

  function logout() {
    clearToken()
    nav('/login')
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">Pertisk <span>Mgmt</span></div>
        <nav className="nav">
          <NavLink to="/" end>Dashboard</NavLink>
          <NavLink to="/clusters">Clusters</NavLink>
          <NavLink to="/providers">Providers</NavLink>
          <NavLink to="/settings">Settings</NavLink>
        </nav>
        <div className="sidebar-foot">
          {user ? `${user.username} · ${user.role}` : '…'}
        </div>
      </aside>
      <div className="main">
        <header className="topbar">
          <div className="muted">Cluster management</div>
          <div className="row-actions">
            <button type="button" className="secondary" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>
              {theme === 'dark' ? 'Light' : 'Dark'}
            </button>
            <button type="button" className="secondary" onClick={logout}>Sign out</button>
          </div>
        </header>
        <div className="content">
          <Outlet />
        </div>
      </div>
    </div>
  )
}
