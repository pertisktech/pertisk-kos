import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import { clearToken, getToken } from './api'
import { useEffect, useState } from 'react'
import { api } from './api'
import { Icon } from './components/Icons'
import { useConfirm } from './components/Confirm'

export default function Layout() {
  const nav = useNavigate()
  const confirm = useConfirm()
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

  async function logout() {
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

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">P</span>
          Pertisk <span>Mgmt</span>
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
        <div className="sidebar-foot">
          {user ? `${user.username} · ${user.role}` : '…'}
        </div>
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
            <button type="button" className="secondary btn-icon" onClick={logout}>
              <Icon name="logout" size={16} /> Sign out
            </button>
          </div>
        </header>
        <div className="content">
          <Outlet />
        </div>
      </div>
    </div>
  )
}
