import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import Modal from '../components/Modal'
import OsBundlePicker, { osBundleReady } from '../components/OsBundlePicker'
import Checkbox from '../components/Checkbox'
import { useConfirm } from '../components/Confirm'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { formatArch } from '../components/ClusterMetaBadges'

function formatBytes(n) {
  const v = Number(n) || 0
  if (v < 1024) return `${v} B`
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(v < 10 * 1024 ? 1 : 0)} KB`
  return `${(v / (1024 * 1024)).toFixed(v < 10 * 1024 * 1024 ? 1 : 0)} MB`
}

function formatWhen(iso) {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

function appendBundle(fd, bundle) {
  if (bundle.zip) {
    fd.append('bundle', bundle.zip)
    return
  }
  fd.append('kernel', bundle.kernel)
  fd.append('initramfs', bundle.initramfs)
  fd.append('manifest.json', bundle.manifest)
  fd.append('manifest.sig', bundle.sig)
}

export default function OsPackages() {
  const nav = useNavigate()
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [clusters, setClusters] = useState([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [uploadOpen, setUploadOpen] = useState(false)
  const [bundle, setBundle] = useState(null)
  const [arch, setArch] = useState('amd64')
  const [applyPkg, setApplyPkg] = useState(null)
  const [selected, setSelected] = useState(() => new Set())

  const load = useCallback(() => {
    Promise.all([
      api('/os-packages').catch((e) => {
        throw e
      }),
      api('/clusters').catch(() => []),
    ])
      .then(([pkgs, cls]) => {
        setList(Array.isArray(pkgs) ? pkgs : [])
        setClusters(Array.isArray(cls) ? cls : [])
        setError('')
      })
      .catch((e) => setError(e.message || 'failed to load OS packages'))
  }, [])

  useEffect(() => {
    load()
  }, [load])

  const applyTargets = useMemo(() => {
    if (!applyPkg) return []
    const a = formatArch(applyPkg.arch)
    return clusters.filter((c) => formatArch(c.arch) === a && c.status !== 'deleting')
  }, [applyPkg, clusters])

  function openApply(pkg) {
    setApplyPkg(pkg)
    setSelected(new Set())
  }

  function toggleCluster(id, on) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (on) next.add(id)
      else next.delete(id)
      return next
    })
  }

  async function upload() {
    if (!osBundleReady(bundle)) return
    setBusy(true)
    setError('')
    try {
      const fd = new FormData()
      fd.append('arch', arch)
      appendBundle(fd, bundle)
      await api('/os-packages', { method: 'POST', body: fd })
      setUploadOpen(false)
      setBundle(null)
      load()
    } catch (e) {
      setError(e.message || 'upload failed')
    } finally {
      setBusy(false)
    }
  }

  async function remove(pkg) {
    const ok = await confirm({
      title: 'Delete OS package',
      message: `Remove ${pkg.version} (${pkg.arch}) from the catalog? Clusters already upgraded keep that OS.`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    try {
      await api(`/os-packages/${pkg.id}`, { method: 'DELETE' })
      load()
    } catch (e) {
      setError(e.message || 'delete failed')
    }
  }

  async function apply() {
    if (!applyPkg || selected.size === 0) return
    const names = applyTargets
      .filter((c) => selected.has(c.id))
      .map((c) => c.name)
      .join(', ')
    const ok = await confirm({
      title: 'OS A/B upgrade',
      message: `Upgrade ${names} to OS ${applyPkg.version}?\n\nWorkers first, then control planes. Kubernetes is not changed.`,
      confirmLabel: 'Start OS upgrade',
      tone: 'primary',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      const res = await api(`/os-packages/${applyPkg.id}/apply`, {
        method: 'POST',
        body: { cluster_ids: [...selected], reboot: true },
      })
      setApplyPkg(null)
      const first = res?.jobs?.[0]
      if (first?.cluster_id) {
        nav(`/clusters/${first.cluster_id}?tab=jobs`)
        return
      }
      load()
    } catch (e) {
      setError(e.message || 'upgrade failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <div className="page-head">
        <h1>
          <Icon name="packages" size={22} /> OS packages
        </h1>
        <button type="button" className="btn btn-icon" onClick={() => setUploadOpen(true)}>
          <Icon name="upload" size={16} /> Upload bundle
        </button>
      </div>
      <p className="muted" style={{ marginTop: '-0.35rem', marginBottom: '1rem' }}>
        Signed A/B images (kernel + initramfs). Pick a version and apply it to matching-arch clusters.
        Kubernetes is not changed.
      </p>
      {error && <div className="error">{error}</div>}
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Version</th>
              <th>Arch</th>
              <th>Size</th>
              <th>Trust key</th>
              <th>Updated</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {list.map((p) => (
              <tr key={p.id}>
                <td>
                  <span className="mono-inline">{p.version}</span>
                </td>
                <td>
                  <span className={`badge arch arch-${formatArch(p.arch)}`}>
                    {formatArch(p.arch)}
                  </span>
                </td>
                <td className="muted">{formatBytes(p.size_bytes)}</td>
                <td>
                  {p.has_trust_pk ? (
                    <span className="badge ready">os-trust.pk</span>
                  ) : (
                    <span className="badge">mgmt fallback</span>
                  )}
                </td>
                <td className="muted" style={{ whiteSpace: 'nowrap' }}>
                  {formatWhen(p.updated_at)}
                </td>
                <td>
                  <div style={{ display: 'flex', gap: '0.35rem', justifyContent: 'flex-end' }}>
                    <button
                      type="button"
                      className="btn-icon"
                      onClick={() => openApply(p)}
                      title="Upgrade clusters"
                    >
                      <Icon name="upgrade" size={14} /> Upgrade
                    </button>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      onClick={() => remove(p)}
                      title="Delete"
                    >
                      <Icon name="trash" size={14} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {list.length === 0 && (
          <p className="muted">
            No packages yet. Upload a zip from <span className="mono-inline">make os-bundle</span>.
          </p>
        )}
      </div>

      <Modal
        open={uploadOpen}
        title="Upload OS bundle"
        icon="upload"
        onClose={() => !busy && setUploadOpen(false)}
      >
        <p className="muted">
          Same signed files as Cluster → Upgrade: kernel, initramfs, manifest.json, manifest.sig
          (or one .zip). Same version+arch replaces the previous package.
        </p>
        <div className="field">
          <label>Arch</label>
          <select value={arch} onChange={(e) => setArch(e.target.value)} disabled={busy}>
            <option value="amd64">amd64</option>
            <option value="arm64">arm64</option>
          </select>
        </div>
        <OsBundlePicker value={bundle} onChange={setBundle} disabled={busy} />
        <div className="form-footer">
          <button type="button" className="secondary" onClick={() => setUploadOpen(false)} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-icon"
            disabled={busy || !osBundleReady(bundle)}
            onClick={upload}
          >
            <Icon name="check" size={16} /> {busy ? 'Uploading…' : 'Save package'}
          </button>
        </div>
      </Modal>

      <Modal
        open={!!applyPkg}
        title={applyPkg ? `Upgrade to ${applyPkg.version}` : 'Upgrade'}
        icon="play"
        onClose={() => !busy && setApplyPkg(null)}
      >
        {applyPkg && (
          <>
            <p className="muted">
              Guest arch must be{' '}
              <span className={`badge arch arch-${formatArch(applyPkg.arch)}`}>
                {formatArch(applyPkg.arch)}
              </span>
              . Workers first, then control planes. STATE and etcd stay.
            </p>
            {applyTargets.length === 0 ? (
              <p className="muted">No {formatArch(applyPkg.arch)} clusters available.</p>
            ) : (
              <ul className="pkg-cluster-list">
                {applyTargets.map((c) => (
                  <li key={c.id}>
                    <Checkbox
                      id={`pkg-c-${c.id}`}
                      checked={selected.has(c.id)}
                      onChange={(on) => toggleCluster(c.id, on)}
                      label={c.name}
                    />
                    <ClusterStatusBadges status={c.status} availability={c.availability} />
                    <Link className="muted" to={`/clusters/${c.id}?tab=upgrade`}>
                      open
                    </Link>
                  </li>
                ))}
              </ul>
            )}
            <div className="form-footer">
              <button type="button" className="secondary" onClick={() => setApplyPkg(null)} disabled={busy}>
                Cancel
              </button>
              <button
                type="button"
                className="btn-icon"
                disabled={busy || selected.size === 0}
                onClick={apply}
              >
                <Icon name="upgrade" size={16} />{' '}
                {busy ? 'Starting…' : `Start OS upgrade (${selected.size})`}
              </button>
            </div>
          </>
        )}
      </Modal>
    </div>
  )
}
