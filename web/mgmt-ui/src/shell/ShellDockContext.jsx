import { createContext, useCallback, useContext, useMemo, useRef, useState } from 'react'

const MGMT_TAB = { id: 'mgmt', kind: 'mgmt', title: 'pertiskctl' }

const ShellDockContext = createContext(null)

function mgmtTitle(index) {
  return index <= 1 ? 'pertiskctl' : `pertiskctl ${index}`
}

export function ShellDockProvider({ children }) {
  const [open, setOpen] = useState(false)
  const [height, setHeight] = useState(300)
  const [tabs, setTabs] = useState([MGMT_TAB])
  const [activeId, setActiveId] = useState(MGMT_TAB.id)
  const mgmtSeq = useRef(0)

  const openMgmtShell = useCallback(() => {
    setTabs((prev) => {
      const existing = prev.find((t) => t.kind === 'mgmt')
      if (existing) {
        setActiveId(existing.id)
        return prev
      }
      setActiveId(MGMT_TAB.id)
      return [MGMT_TAB, ...prev]
    })
    setOpen(true)
  }, [])

  /** Always create a fresh pertiskctl session (dock + button). */
  const newMgmtTab = useCallback(() => {
    const id = `mgmt:${++mgmtSeq.current}`
    setTabs((prev) => {
      const count = prev.filter((t) => t.kind === 'mgmt').length + 1
      return [...prev, { id, kind: 'mgmt', title: mgmtTitle(count) }]
    })
    setActiveId(id)
    setOpen(true)
  }, [])

  const openClusterShell = useCallback((clusterId, clusterName) => {
    if (!clusterId) return
    const id = `cluster:${clusterId}`
    const title = clusterName || clusterId
    setTabs((prev) => {
      if (prev.some((t) => t.id === id)) return prev
      return [...prev, { id, kind: 'cluster', clusterId, title }]
    })
    setActiveId(id)
    setOpen(true)
  }, [])

  const closeTab = useCallback((id) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id)
      if (next.length === 0) {
        setOpen(false)
        setActiveId(MGMT_TAB.id)
        return [MGMT_TAB]
      }
      setActiveId((cur) => (cur === id ? next[next.length - 1].id : cur))
      return next
    })
  }, [])

  const toggle = useCallback(() => {
    setOpen((v) => !v)
  }, [])

  const value = useMemo(
    () => ({
      open,
      setOpen,
      height,
      setHeight,
      tabs,
      activeId,
      setActiveId,
      openMgmtShell,
      newMgmtTab,
      openClusterShell,
      closeTab,
      toggle,
    }),
    [open, height, tabs, activeId, openMgmtShell, newMgmtTab, openClusterShell, closeTab, toggle],
  )

  return <ShellDockContext.Provider value={value}>{children}</ShellDockContext.Provider>
}

export function useShellDock() {
  const ctx = useContext(ShellDockContext)
  if (!ctx) {
    throw new Error('useShellDock must be used within ShellDockProvider')
  }
  return ctx
}
