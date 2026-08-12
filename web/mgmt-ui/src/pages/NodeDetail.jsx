import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  Area,
  AreaChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { NodeStatusBadges } from '../components/NodeStatusBadges'
import ColorLogViewer from '../components/ColorLogViewer'

const POLL_MS = 4000
const LOG_POLL_MS = 3000
const MAX_POINTS = 60
const LOG_SERVICES = ['pertiskd', 'containerd', 'kubelet', 'dmesg']
const LOG_TAIL = 200

function formatHw(node) {
  if (!node) return '—'
  const cores = node.cores ?? '—'
  const mem = node.memory != null ? `${node.memory} MB` : '—'
  const disk = node.disk_gb != null ? `${node.disk_gb} GiB` : '—'
  return `${cores} vCPU · ${mem} · ${disk}`
}

function formatTime(ts) {
  const d = new Date(ts)
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function gauge(metrics, key) {
  const v = metrics?.gauges?.[key]
  return typeof v === 'number' ? v : null
}

function HealthPill({ label, ok, value }) {
  const cls = ok === true ? 'up' : ok === false ? 'down' : 'unknown'
  return (
    <div className={`health-pill ${cls}`}>
      <span className="health-pill-label">{label}</span>
      <span className="health-pill-value">{value ?? '—'}</span>
    </div>
  )
}

function ChartCard({ title, error, children }) {
  return (
    <div className="card chart-card">
      <h2 className="card-title">{title}</h2>
      {error && <p className="muted chart-error">{error}</p>}
      <div className="chart-wrap">{children}</div>
    </div>
  )
}

const tooltipStyle = {
  background: 'var(--bg-elevated)',
  border: '1px solid var(--border)',
  borderRadius: 8,
  fontSize: 12,
}

export default function NodeDetail() {
  const { id: clusterId, nid } = useParams()
  const [status, setStatus] = useState(null)
  const [clusterName, setClusterName] = useState('')
  const [err, setErr] = useState(null)
  const [series, setSeries] = useState([])
  const lastApi = useRef({ total: null, sum: null, count: null })

  const [logService, setLogService] = useState('pertiskd')
  const [logText, setLogText] = useState('')
  const [logSource, setLogSource] = useState('')
  const [logError, setLogError] = useState(null)
  const [followLog, setFollowLog] = useState(true)

  const [attest, setAttest] = useState(null)
  const [attestErr, setAttestErr] = useState(null)
  const [attestBusy, setAttestBusy] = useState(null)

  const load = useCallback(async () => {
    try {
      const [snap, clusterRes] = await Promise.all([
        api(`/clusters/${clusterId}/nodes/${nid}/status`),
        api(`/clusters/${clusterId}`).catch(() => null),
      ])
      setStatus(snap)
      setErr(null)
      if (clusterRes?.cluster?.name) setClusterName(clusterRes.cluster.name)

      const now = Date.now()
      const apiMetrics = snap.metrics?.api
      const total = apiMetrics?.requests_total ?? 0
      const sum = apiMetrics?.duration_sum_seconds ?? 0
      const count = apiMetrics?.duration_count ?? 0
      let reqRate = 0
      let avgMs = null
      const prev = lastApi.current
      if (prev.total != null) {
        const dt = Math.max((now - (prev.t || now - POLL_MS)) / 1000, 0.001)
        reqRate = Math.max(0, (total - prev.total) / dt)
        const dCount = count - (prev.count ?? 0)
        const dSum = sum - (prev.sum ?? 0)
        if (dCount > 0) avgMs = (dSum / dCount) * 1000
      } else if (count > 0) {
        avgMs = (sum / count) * 1000
      }
      lastApi.current = { total, sum, count, t: now }

      const point = {
        t: now,
        label: formatTime(now),
        ready: gauge(snap.metrics, 'pertisk_node_ready') ?? (snap.health?.ready ? 1 : 0),
        containerd: gauge(snap.metrics, 'pertisk_containerd_up')
          ?? (snap.health?.containerd === 'up' ? 1 : 0),
        kubelet: gauge(snap.metrics, 'pertisk_kubelet_up')
          ?? (snap.health?.kubelet === 'up' ? 1 : 0),
        bootOk: gauge(snap.metrics, 'pertisk_boot_ok'),
        reqRate: Number(reqRate.toFixed(2)),
        avgMs: avgMs != null ? Number(avgMs.toFixed(2)) : null,
        cpu: snap.resources?.cpu_percent ?? null,
        memory: snap.resources?.memory_percent ?? null,
      }

      setSeries((prevSeries) => {
        const next = [...prevSeries, point]
        return next.length > MAX_POINTS ? next.slice(next.length - MAX_POINTS) : next
      })
    } catch (e) {
      setErr(e.message || String(e))
    }
  }, [clusterId, nid])

  useEffect(() => {
    setSeries([])
    lastApi.current = { total: null, sum: null, count: null }
    load()
    const t = setInterval(load, POLL_MS)
    return () => clearInterval(t)
  }, [load])

  const loadLogs = useCallback(async () => {
    try {
      const res = await api(
        `/clusters/${clusterId}/nodes/${nid}/logs?service=${encodeURIComponent(logService)}&tail=${LOG_TAIL}`,
      )
      if (res.error) {
        setLogError(res.error)
        setLogText('')
        setLogSource('')
        return
      }
      setLogError(null)
      setLogSource(res.source || '')
      setLogText((res.lines || []).join('\n'))
    } catch (e) {
      setLogError(e.message || String(e))
    }
  }, [clusterId, nid, logService])

  useEffect(() => {
    setLogText('')
    setLogError(null)
    loadLogs()
    const t = setInterval(loadLogs, LOG_POLL_MS)
    return () => clearInterval(t)
  }, [loadLogs])

  const loadAttest = useCallback(async () => {
    try {
      const res = await api(`/clusters/${clusterId}/nodes/${nid}/attestation`)
      setAttest(res)
      setAttestErr(null)
    } catch (e) {
      setAttestErr(e.message || String(e))
    }
  }, [clusterId, nid])

  useEffect(() => {
    setAttest(null)
    setAttestErr(null)
    loadAttest()
  }, [loadAttest])

  async function runAttest(action) {
    setAttestBusy(action)
    setAttestErr(null)
    try {
      const res = await api(`/clusters/${clusterId}/nodes/${nid}/attestation/${action}`, {
        method: 'POST',
      })
      setAttest(res)
    } catch (e) {
      setAttestErr(e.message || String(e))
    } finally {
      setAttestBusy(null)
    }
  }

  const node = status?.node
  const health = status?.health
  const chartData = useMemo(() => series, [series])

  const healthOk = (v) => {
    if (v === true || v === 1 || v === 'up') return true
    if (v === false || v === 0 || v === 'down') return false
    return null
  }

  return (
    <div>
      <div className="page-head">
        <div>
          <p className="muted breadcrumb">
            <Link to={`/clusters/${clusterId}`}>{clusterName || 'Cluster'}</Link>
            {' / '}
            <Link to={`/clusters/${clusterId}?tab=nodes`}>Nodes</Link>
            {' / '}
            <span>{node?.name || '…'}</span>
          </p>
          <h1>
            <Icon name="worker" size={22} />
            {node?.name || 'Node'}
            {node && (
              <>
                {' '}
                <span className="badge">{node.role === 'controlplane' ? 'CP' : 'worker'}</span>
                {' '}
                <NodeStatusBadges
                  status={node.status}
                  availability={
                    health?.error
                      ? 'offline'
                      : health?.ready != null || health?.containerd
                        ? 'online'
                        : node.availability || 'unknown'
                  }
                />
              </>
            )}
          </h1>
        </div>
        <Link className="btn secondary btn-icon" to={`/clusters/${clusterId}?tab=nodes`}>
          <Icon name="clusters" size={16} /> Back to cluster
        </Link>
      </div>

      {err && <p className="banner danger">{err}</p>}

      {node && (
        <div className="grid-stats node-info-stats">
          <div className="stat">
            <div className="label">VMID</div>
            <div className="value mono-inline">{node.vmid ?? '—'}</div>
          </div>
          <div className="stat">
            <div className="label">IPv4</div>
            <div className="value mono-inline">{node.ip || '—'}</div>
          </div>
          <div className="stat">
            <div className="label">IPv6</div>
            <div className="value mono-inline">{node.ip6 || '—'}</div>
          </div>
          <div className="stat">
            <div className="label">K8s</div>
            <div className="value mono-inline">{node.k8s_version || '—'}</div>
          </div>
          <div className="stat">
            <div className="label">Hardware</div>
            <div className="value" style={{ fontSize: '0.95rem' }}>{formatHw(node)}</div>
          </div>
        </div>
      )}

      <div className="card">
        <h2 className="card-title"><Icon name="play" size={18} /> Live health</h2>
        {health?.error ? (
          <p className="muted">{health.error}</p>
        ) : (
          <div className="health-pills">
            <HealthPill
              label="Ready"
              ok={healthOk(health?.ready)}
              value={health?.ready == null ? null : health.ready ? 'yes' : 'no'}
            />
            <HealthPill
              label="containerd"
              ok={healthOk(health?.containerd)}
              value={health?.containerd}
            />
            <HealthPill
              label="kubelet"
              ok={healthOk(health?.kubelet)}
              value={health?.kubelet}
            />
          </div>
        )}
        {health?.message && <p className="muted" style={{ marginTop: '0.75rem' }}>{health.message}</p>}
      </div>

      <div className="card">
        <h2 className="card-title">TPM Quote attestation</h2>
        {attestErr && <p className="banner danger">{attestErr}</p>}
        {attest ? (
          <div className="health-pills">
            <HealthPill
              label="Enrolled"
              ok={attest.enrolled}
              value={attest.enrolled ? 'yes' : 'no'}
            />
            <HealthPill
              label="AK"
              ok={attest.enrolled ? true : null}
              value={attest.ak_fingerprint ? `…${attest.ak_fingerprint}` : '—'}
            />
            <HealthPill
              label="EK"
              ok={
                attest.ek_chain_status === 'ok'
                  ? true
                  : attest.ek_chain_status === 'failed'
                    ? false
                    : attest.ek_fingerprint
                      ? true
                      : null
              }
              value={
                attest.ek_fingerprint
                  ? `…${attest.ek_fingerprint}${attest.ek_chain_status ? ` (${attest.ek_chain_status})` : ''}`
                  : '—'
              }
            />
            {attest.ok != null && (
              <HealthPill
                label="Verify"
                ok={attest.ok}
                value={attest.ok ? 'ok' : 'fail'}
              />
            )}
          </div>
        ) : (
          <p className="muted">Loading attestation status…</p>
        )}
        {attest?.message && (
          <p className="muted" style={{ marginTop: '0.75rem' }}>{attest.message}</p>
        )}
        {attest?.ak_enrolled_at && (
          <p className="muted" style={{ marginTop: '0.25rem' }}>
            Enrolled at {attest.ak_enrolled_at}
          </p>
        )}
        <div className="row-actions" style={{ marginTop: '0.75rem', display: 'flex', gap: '0.5rem' }}>
          <button
            type="button"
            className="btn secondary"
            disabled={attestBusy || !node?.ip}
            onClick={() => runAttest('enroll')}
          >
            {attestBusy === 'enroll' ? 'Enrolling…' : 'Enroll AK'}
          </button>
          <button
            type="button"
            className="btn secondary"
            disabled={attestBusy || !node?.ip || !attest?.enrolled}
            onClick={() => runAttest('verify')}
          >
            {attestBusy === 'verify' ? 'Verifying…' : 'Verify Quote'}
          </button>
        </div>
        {!node?.ip && (
          <p className="muted" style={{ marginTop: '0.5rem' }}>
            Node needs an IPv4 before Quote enroll/verify.
          </p>
        )}
      </div>

      <div className="chart-grid">
        <ChartCard title="Node readiness" error={status?.metrics?.error}>
          <ResponsiveContainer width="100%" height={220}>
            <AreaChart data={chartData}>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
              <XAxis dataKey="label" tick={{ fill: 'var(--text-muted)', fontSize: 11 }} minTickGap={24} />
              <YAxis domain={[0, 1]} ticks={[0, 1]} tick={{ fill: 'var(--text-muted)', fontSize: 11 }} width={28} />
              <Tooltip contentStyle={tooltipStyle} />
              <Legend />
              <Area type="stepAfter" dataKey="ready" name="ready" stroke="var(--success)" fill="rgba(74,222,155,0.2)" strokeWidth={2} />
              <Area type="stepAfter" dataKey="containerd" name="containerd" stroke="var(--accent)" fill="rgba(154,123,247,0.15)" strokeWidth={2} />
              <Area type="stepAfter" dataKey="kubelet" name="kubelet" stroke="var(--warning)" fill="rgba(251,191,36,0.12)" strokeWidth={2} />
            </AreaChart>
          </ResponsiveContainer>
        </ChartCard>

        <ChartCard title="Machine API" error={status?.metrics?.error}>
          <ResponsiveContainer width="100%" height={220}>
            <LineChart data={chartData}>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
              <XAxis dataKey="label" tick={{ fill: 'var(--text-muted)', fontSize: 11 }} minTickGap={24} />
              <YAxis yAxisId="rate" tick={{ fill: 'var(--text-muted)', fontSize: 11 }} width={40} />
              <YAxis yAxisId="lat" orientation="right" tick={{ fill: 'var(--text-muted)', fontSize: 11 }} width={44} />
              <Tooltip contentStyle={tooltipStyle} />
              <Legend />
              <Line yAxisId="rate" type="monotone" dataKey="reqRate" name="req/s" stroke="var(--accent)" strokeWidth={2} dot={false} />
              <Line yAxisId="lat" type="monotone" dataKey="avgMs" name="avg ms" stroke="var(--success)" strokeWidth={2} dot={false} connectNulls />
            </LineChart>
          </ResponsiveContainer>
          {status?.metrics?.api && (
            <p className="muted chart-footnote">
              total requests {status.metrics.api.requests_total}
              {status.metrics.api.duration_count > 0 && (
                <> · lifetime avg{' '}
                  {((status.metrics.api.duration_sum_seconds / status.metrics.api.duration_count) * 1000).toFixed(2)} ms
                </>
              )}
            </p>
          )}
        </ChartCard>

        <ChartCard title="CPU / memory (metrics-server)" error={status?.resources?.error}>
          <ResponsiveContainer width="100%" height={220}>
            <LineChart data={chartData}>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
              <XAxis dataKey="label" tick={{ fill: 'var(--text-muted)', fontSize: 11 }} minTickGap={24} />
              <YAxis domain={[0, 100]} tick={{ fill: 'var(--text-muted)', fontSize: 11 }} width={36} unit="%" />
              <Tooltip contentStyle={tooltipStyle} />
              <Legend />
              <Line type="monotone" dataKey="cpu" name="CPU %" stroke="var(--accent)" strokeWidth={2} dot={false} connectNulls />
              <Line type="monotone" dataKey="memory" name="Mem %" stroke="var(--warning)" strokeWidth={2} dot={false} connectNulls />
            </LineChart>
          </ResponsiveContainer>
          {status?.resources && !status.resources.error && (
            <p className="muted chart-footnote">
              {status.resources.cpu || '—'} CPU · {status.resources.memory || '—'} memory
            </p>
          )}
        </ChartCard>
      </div>

      <p className="muted" style={{ marginTop: '0.5rem' }}>
        Charts keep ~{MAX_POINTS} samples in this browser session (poll every {POLL_MS / 1000}s). Refresh clears history.
      </p>

      <div className="card node-logs-card">
        <div className="section-head log-head">
          <h2 className="card-title" style={{ margin: 0 }}>
            <Icon name="providers" size={18} /> Logs
          </h2>
          <div className="row-actions node-log-actions">
            <select
              className="log-service-select"
              value={logService}
              onChange={(e) => setLogService(e.target.value)}
              aria-label="Log service"
            >
              {LOG_SERVICES.map((s) => (
                <option key={s} value={s}>{s}</option>
              ))}
            </select>
            <button
              type="button"
              className="secondary btn-icon"
              onClick={() => loadLogs()}
              title="Refresh now"
            >
              Refresh
            </button>
            <button
              type="button"
              className={`secondary btn-icon ${followLog ? 'follow-on' : ''}`}
              onClick={() => setFollowLog(true)}
              title="Follow latest output"
            >
              {followLog ? 'Following' : 'Follow'}
            </button>
          </div>
        </div>
        {logSource && !logError && (
          <p className="muted chart-footnote">{logService} · {logSource}</p>
        )}
        {logError ? (
          <p className="muted chart-error">{logError}</p>
        ) : (
          <ColorLogViewer
            text={logText}
            empty="—"
            follow={followLog}
            onFollowChange={setFollowLog}
            className="node-log-box"
            aria-label={`${logService} logs`}
          />
        )}
      </div>
    </div>
  )
}
