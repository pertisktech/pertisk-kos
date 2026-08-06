import Xterm from '../../components/Xterm'

/**
 * Host-level shell for installing apps (kubectl / helm) against this cluster.
 */
export default function ShellTab({ clusterId, clusterName, ready }) {
  if (!ready) {
    return (
      <div className="tab-body">
        <p className="muted">
          Shell is available when the cluster status is <span className="badge ready">ready</span>
          {' '}and a kubeconfig has been stored. Use it to run{' '}
          <code className="mono-inline">kubectl</code> / <code className="mono-inline">helm</code>
          {' '}on the management host.
        </p>
      </div>
    )
  }

  return (
    <div className="tab-body tab-body-fill shell-tab">
      <div className="section-head">
        <div>
          <h3 className="section-label">Shell</h3>
          <p className="muted">
            OS shell on the management host with <code className="mono-inline">KUBECONFIG</code> set
            for this cluster. Install apps with kubectl or helm (both must be on the mgmt host PATH).
          </p>
        </div>
      </div>
      <div className="k8s-shell-dock k8s-shell-dock-fill">
        <Xterm clusterId={clusterId} clusterName={clusterName} />
      </div>
    </div>
  )
}
