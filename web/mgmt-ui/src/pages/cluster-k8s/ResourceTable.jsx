import { Icon } from '../../components/Icons'

function statusClass(status) {
  const s = (status || '').toLowerCase()
  if (['running', 'active', 'complete', 'succeeded', 'ready', 'bound', 'tls', 'clusterip', 'normal'].includes(s)) {
    return 'ready'
  }
  if (['pending', 'progressing', 'suspended', 'warning', 'nodeport', 'loadbalancer'].includes(s)) {
    return 'provisioning'
  }
  if (['failed', 'stopped', 'error', 'crashloopbackoff', 'notready', 'lost'].includes(s)) {
    return 'error'
  }
  return ''
}

function extraColumns(kind) {
  if (kind === 'pods') return ['Restarts', 'Node']
  if (kind === 'cronjobs') return ['Schedule']
  if (kind === 'services') return ['Ports']
  if (kind === 'ingresses') return ['Hosts']
  if (kind === 'persistentvolumeclaims') return ['StorageClass']
  if (kind === 'events') return ['Object', 'Message']
  if (kind === 'nodes') return ['Roles', 'Version']
  if (['deployments', 'statefulsets', 'daemonsets'].includes(kind)) return ['Images']
  return []
}

function extraCells(kind, r) {
  if (kind === 'pods') {
    return [
      <td key="re">{r.restarts ?? 0}</td>,
      <td key="node" className="k8s-td-muted">{r.node || '—'}</td>,
    ]
  }
  if (kind === 'cronjobs') {
    return [<td key="sch" className="mono-inline">{r.schedule || '—'}</td>]
  }
  if (kind === 'services') {
    return [<td key="ports" className="k8s-td-muted">{r.ports || '—'}</td>]
  }
  if (kind === 'ingresses') {
    return [<td key="hosts" className="k8s-td-muted">{r.hosts || '—'}</td>]
  }
  if (kind === 'persistentvolumeclaims') {
    return [<td key="sc" className="mono-inline k8s-td-muted">{r.storageClass || '—'}</td>]
  }
  if (kind === 'events') {
    return [
      <td key="obj" className="mono-inline">{r.object || '—'}</td>,
      <td key="msg" className="k8s-td-muted k8s-td-clamp">{r.message || '—'}</td>,
    ]
  }
  if (kind === 'nodes') {
    return [
      <td key="roles">{r.roles || '—'}</td>,
      <td key="ver" className="mono-inline">{r.version || '—'}</td>,
    ]
  }
  if (['deployments', 'statefulsets', 'daemonsets'].includes(kind)) {
    return [
      <td key="img" className="k8s-td-muted k8s-td-clamp">
        {(r.images || []).map((img) => String(img).split('@')[0]).slice(0, 2).join(', ') || '—'}
      </td>,
    ]
  }
  return []
}

function rowKey(r) {
  return `${r.namespace || ''}/${r.name}`
}

export default function ResourceTable({
  kind,
  rows,
  selectedKey,
  onSelect,
  onLogs,
  onExec,
}) {
  const extras = extraColumns(kind)
  const isPods = kind === 'pods'
  const clusterScoped = kind === 'nodes'

  return (
    <div className="k8s-table-shell">
      <div className="k8s-table-meta">
        <span>
          Total: <strong>{rows.length}</strong> records
        </span>
      </div>
      <div className="table-wrap k8s-table">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              {!clusterScoped && <th>Namespace</th>}
              <th>Status</th>
              <th>Ready</th>
              {extras.map((h) => (
                <th key={h}>{h}</th>
              ))}
              <th>Age</th>
              {isPods && <th className="k8s-th-actions">Actions</th>}
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 && (
              <tr>
                <td colSpan={5 + extras.length + (isPods ? 1 : 0)} className="k8s-td-empty">
                  No data available
                </td>
              </tr>
            )}
            {rows.map((r) => {
              const key = rowKey(r)
              const selected = selectedKey === key
              return (
                <tr
                  key={key}
                  className={`k8s-row${selected ? ' is-selected' : ''}`}
                  onClick={() => onSelect?.(r)}
                >
                  <td className="k8s-td-name">{r.name}</td>
                  {!clusterScoped && <td className="k8s-td-muted">{r.namespace || '—'}</td>}
                  <td>
                    <span className={`k8s-status ${statusClass(r.status)}`}>{r.status}</span>
                  </td>
                  <td className="mono-inline">{kind === 'cronjobs' ? '—' : r.ready}</td>
                  {extraCells(kind, r)}
                  <td className="k8s-td-muted">{r.age}</td>
                  {isPods && (
                    <td className="k8s-td-actions" onClick={(e) => e.stopPropagation()}>
                      <button
                        type="button"
                        className="k8s-icon-btn"
                        title="Logs"
                        onClick={() => onLogs?.(r)}
                      >
                        <Icon name="logs" size={14} />
                      </button>
                      <button
                        type="button"
                        className="k8s-icon-btn"
                        title="Exec"
                        onClick={() => onExec?.(r)}
                      >
                        <Icon name="terminal" size={14} />
                      </button>
                    </td>
                  )}
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </div>
  )
}
