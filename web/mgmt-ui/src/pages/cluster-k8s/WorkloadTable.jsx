import { Icon } from '../../components/Icons'

export default function WorkloadTable({
  kind,
  rows,
  onScale,
  onRestart,
  onDelete,
}) {
  const isPods = kind === 'pods'
  const isDeploy = kind === 'deployments'

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th>Namespace</th>
            <th>Status</th>
            <th>Ready</th>
            {isPods && <th>Restarts</th>}
            {isPods && <th>Node</th>}
            {!isPods && kind !== 'jobs' && kind !== 'cronjobs' && <th>Images</th>}
            {kind === 'cronjobs' && <th>Schedule</th>}
            <th>Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td colSpan={8} className="muted">
                No resources
              </td>
            </tr>
          )}
          {rows.map((r) => (
            <tr key={`${r.namespace}/${r.name}`}>
              <td className="mono-inline">{r.name}</td>
              <td>{r.namespace}</td>
              <td>
                <span className={`badge ${statusClass(r.status)}`}>{r.status}</span>
              </td>
              <td className="mono-inline">{kind === 'cronjobs' ? '—' : r.ready}</td>
              {isPods && <td>{r.restarts ?? 0}</td>}
              {isPods && <td className="mono-inline muted">{r.node || '—'}</td>}
              {!isPods && kind !== 'jobs' && kind !== 'cronjobs' && (
                <td className="muted" style={{ maxWidth: 220 }}>
                  {(r.images || []).slice(0, 2).join(', ') || '—'}
                </td>
              )}
              {kind === 'cronjobs' && <td className="mono-inline">{r.schedule || r.ready}</td>}
              <td className="muted">{r.age}</td>
              <td className="row-actions">
                {isDeploy && (
                  <>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      title="Scale"
                      onClick={() => onScale?.(r)}
                    >
                      Scale
                    </button>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      title="Restart"
                      onClick={() => onRestart?.(r)}
                    >
                      Restart
                    </button>
                  </>
                )}
                <button
                  type="button"
                  className="danger btn-icon"
                  title="Delete"
                  onClick={() => onDelete?.(r)}
                >
                  <Icon name="trash" size={14} />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function statusClass(status) {
  const s = (status || '').toLowerCase()
  if (['running', 'active', 'complete', 'succeeded'].includes(s)) return 'ready'
  if (['pending', 'progressing', 'suspended'].includes(s)) return 'provisioning'
  if (['failed', 'stopped', 'error', 'crashloopbackoff'].includes(s)) return 'error'
  return ''
}
