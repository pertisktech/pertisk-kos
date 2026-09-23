import { Icon } from '../../components/Icons'
import { useShellDock } from '../../shell/ShellDockContext'

/**
 * Opens this cluster's kubectl/helm host shell in the bottom multi-tab dock.
 */
export default function ShellTab({ clusterId, clusterName, ready }) {
  const { openClusterShell, openMgmtShell } = useShellDock()

  if (!ready) {
    return (
      <div className="tab-body">
        <p className="muted">
          Shell is available when the cluster status is <span className="badge ready">ready</span>
          {' '}and a kubeconfig has been stored. Use it to run{' '}
          <code className="mono-inline">kubectl</code> / <code className="mono-inline">helm</code>
          {' '}on the management host.
        </p>
        <p className="muted" style={{ marginTop: '0.75rem' }}>
          The management <code className="mono-inline">pertiskctl</code> shell is always available
          from the bottom dock.
        </p>
        <button type="button" className="secondary btn-icon" onClick={openMgmtShell}>
          <Icon name="terminal" size={16} /> Open pertiskctl shell
        </button>
      </div>
    )
  }

  return (
    <div className="tab-body shell-tab-open">
      <div className="section-head">
        <div>
          <h3 className="section-label">Shell</h3>
          <p className="muted">
            Opens a tab in the bottom dock: OS shell on the management host with{' '}
            <code className="mono-inline">KUBECONFIG</code> set for this cluster (kubectl / helm).
            Switch to the <code className="mono-inline">pertiskctl</code> tab for management ops.
          </p>
        </div>
      </div>
      <div className="shell-open-actions">
        <button
          type="button"
          className="btn btn-icon"
          onClick={() => openClusterShell(clusterId, clusterName)}
        >
          <Icon name="terminal" size={16} /> Open cluster shell
        </button>
        <button type="button" className="secondary btn-icon" onClick={openMgmtShell}>
          <Icon name="settings" size={16} /> pertiskctl
        </button>
      </div>
    </div>
  )
}
