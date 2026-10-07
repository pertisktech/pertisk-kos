import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from './Icons'

const STATIC_ROUTES = [
  { id: 'dash', label: 'Dashboard', to: '/', icon: 'dashboard', group: 'Go to' },
  { id: 'clusters', label: 'Clusters', to: '/clusters', icon: 'clusters', group: 'Go to' },
  { id: 'machines', label: 'Machines', to: '/machines', icon: 'machines', group: 'Go to' },
  { id: 'providers', label: 'Providers', to: '/providers', icon: 'providers', group: 'Go to' },
  { id: 'images', label: 'Images', to: '/images', icon: 'disk', group: 'Go to' },
  { id: 'os', label: 'OS packages', to: '/os-packages', icon: 'packages', group: 'Go to' },
  { id: 'templates', label: 'Templates', to: '/templates', icon: 'templates', group: 'Go to' },
  { id: 'users', label: 'Users', to: '/users', icon: 'users', group: 'Go to' },
  { id: 'audit', label: 'Audit', to: '/audit', icon: 'audit', group: 'Go to' },
  { id: 'settings', label: 'Settings', to: '/settings', icon: 'settings', group: 'Go to' },
  { id: 'new-cluster', label: 'New cluster', to: '/clusters?new=1', icon: 'plus', group: 'Actions' },
]

/**
 * Lightweight ⌘K / Ctrl+K command palette — jump to routes, clusters, providers.
 */
export default function CommandPalette({ open, onClose }) {
  const nav = useNavigate()
  const inputRef = useRef(null)
  const [q, setQ] = useState('')
  const [clusters, setClusters] = useState([])
  const [providers, setProviders] = useState([])
  const [active, setActive] = useState(0)

  useEffect(() => {
    if (!open) return undefined
    setQ('')
    setActive(0)
    const t = requestAnimationFrame(() => inputRef.current?.focus())
    let cancelled = false
    Promise.all([
      api('/clusters').catch(() => ({ data: [] })),
      api('/providers').catch(() => ({ data: [] })),
    ]).then(([c, p]) => {
      if (cancelled) return
      setClusters(Array.isArray(c) ? c : Array.isArray(c?.data) ? c.data : [])
      setProviders(Array.isArray(p) ? p : Array.isArray(p?.data) ? p.data : [])
    })
    return () => {
      cancelled = true
      cancelAnimationFrame(t)
    }
  }, [open])

  const items = useMemo(() => {
    const needle = q.trim().toLowerCase()
    const match = (s) => !needle || String(s || '').toLowerCase().includes(needle)
    const out = []
    for (const r of STATIC_ROUTES) {
      if (match(r.label)) out.push({ ...r, type: 'route' })
    }
    for (const c of clusters) {
      if (match(c.name) || match(c.id)) {
        out.push({
          id: `c:${c.id}`,
          label: c.name || c.id,
          hint: c.status || 'cluster',
          to: `/clusters/${c.id}`,
          icon: 'clusters',
          group: 'Clusters',
          type: 'cluster',
        })
      }
    }
    for (const p of providers) {
      if (match(p.name) || match(p.id)) {
        out.push({
          id: `p:${p.id}`,
          label: p.name || p.id,
          hint: p.kind || 'provider',
          to: `/providers/${p.id}`,
          icon: 'providers',
          group: 'Providers',
          type: 'provider',
        })
      }
    }
    if (needle && !out.some((i) => i.to?.startsWith('/machines'))) {
      out.push({
        id: 'search-machines',
        label: `Search machines for “${q.trim()}”`,
        to: `/machines?q=${encodeURIComponent(q.trim())}`,
        icon: 'search',
        group: 'Search',
        type: 'search',
      })
    }
    return out.slice(0, 40)
  }, [q, clusters, providers])

  useEffect(() => {
    setActive(0)
  }, [q])

  const go = useCallback(
    (item) => {
      if (!item?.to) return
      onClose?.()
      nav(item.to)
    },
    [nav, onClose],
  )

  useEffect(() => {
    if (!open) return undefined
    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose?.()
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setActive((i) => Math.min(i + 1, Math.max(items.length - 1, 0)))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((i) => Math.max(i - 1, 0))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        go(items[active])
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [open, items, active, go, onClose])

  if (!open) return null

  let lastGroup = null

  return (
    <div className="cmdk-root" role="presentation">
      <button type="button" className="cmdk-backdrop" aria-label="Close" onClick={onClose} />
      <div className="cmdk-panel" role="dialog" aria-modal="true" aria-label="Command palette">
        <div className="cmdk-input-row">
          <Icon name="search" size={16} />
          <input
            ref={inputRef}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Jump to cluster, provider, or page…"
            aria-label="Command search"
          />
          <kbd className="cmdk-kbd">esc</kbd>
        </div>
        <ul className="cmdk-list" role="listbox">
          {items.length === 0 && <li className="cmdk-empty muted">No matches</li>}
          {items.map((item, idx) => {
            const showGroup = item.group !== lastGroup
            lastGroup = item.group
            return (
              <li key={item.id}>
                {showGroup && <div className="cmdk-group">{item.group}</div>}
                <button
                  type="button"
                  role="option"
                  aria-selected={idx === active}
                  className={`cmdk-item${idx === active ? ' active' : ''}`}
                  onMouseEnter={() => setActive(idx)}
                  onClick={() => go(item)}
                >
                  <Icon name={item.icon || 'search'} size={14} />
                  <span className="cmdk-item-label">{item.label}</span>
                  {item.hint && <span className="cmdk-item-hint muted">{item.hint}</span>}
                </button>
              </li>
            )
          })}
        </ul>
      </div>
    </div>
  )
}
