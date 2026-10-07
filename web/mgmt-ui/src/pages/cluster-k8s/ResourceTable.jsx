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
      <td key="node" className="mono-inline muted">{r.node || '—'}</td>,
    ]
  }
  if (kind === 'cronjobs') {
    return [<td key="sch" className="mono-inline">{r.schedule || '—'}</td>]
  }
  if (kind === 'services') {
    return [<td key="ports" className="muted">{r.ports || '—'}</td>]
  }
  if (kind === 'ingresses') {
    return [<td key="hosts" className="muted">{r.hosts || '—'}</td>]
  }
  if (kind === 'persistentvolumeclaims') {
    return [<td key="sc" className="mono-inline muted">{r.storageClass || '—'}</td>]
  }
  if (kind === 'events') {
    return [
      <td key="obj" className="mono-inline">{r.object || '—'}</td>,
      <td key="msg" className="muted" style={{ maxWidth: 280 }}>{r.message || '—'}</td>,
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
      <td key="img" className="muted" style={{ maxWidth: 220 }}>
        {(r.images || []).slice(0, 2).join(', ') || '—'}
      </td>,
    ]
  }
  return []
}

export default function ResourceTable({
  kind,
  rows,
  onSelect,
  onScale,
  onRestart,
  onDelete,
  onLogs,
  onExec,
}) {
  const extras = extraColumns(kind)
  const isDeploy = kind === 'deployments'
  const isPods = kind === 'pods'
  const clusterScoped = kind === 'nodes'
  const canDelete = kind !== 'events' && kind !== 'nodes'

  return (
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
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td colSpan={6 + extras.length} className="muted">
                No resources
              </td>
            </tr>
          )}
          {rows.map((r) => (
            <tr
              key={`${r.namespace || ''}/${r.name}`}
              className="k8s-row-clickable"
              onClick={() => onSelect?.(r)}
            >
              <td className="mono-inline">{r.name}</td>
              {!clusterScoped && <td>{r.namespace || '—'}</td>}
              <td>
                <span className={`badge ${statusClass(r.status)}`}>{r.status}</span>
              </td>
              <td className="mono-inline">{kind === 'cronjobs' ? '—' : r.ready}</td>
              {extraCells(kind, r)}
              <td className="muted">{r.age}</td>
              <td className="row-actions" onClick={(e) => e.stopPropagation()}>
                {isPods && (
                  <>
                    <button type="button" className="secondary btn-icon" title="Logs" onClick={() => onLogs?.(r)}>
                      <Icon name="logs" size={14} />
                    </button>
                    <button type="button" className="secondary btn-icon" title="Exec" onClick={() => onExec?.(r)}>
                      <Icon name="terminal" size={14} />
                    </button>
                  </>
                )}
                {isDeploy && (
                  <>
                    <button type="button" className="secondary btn-icon" title="Scale" onClick={() => onScale?.(r)}>
                      Scale
                    </button>
                    <button type="button" className="secondary btn-icon" title="Restart" onClick={() => onRestart?.(r)}>
                      Restart
                    </button>
                  </>
                )}
                {canDelete && (
                  <button type="button" className="danger btn-icon" title="Delete" onClick={() => onDelete?.(r)}>
                    <Icon name="trash" size={14} />
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
