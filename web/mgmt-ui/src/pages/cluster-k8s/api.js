import { api, getToken } from '../../api'

export const RESOURCE_GROUPS = [
  {
    id: 'workloads',
    label: 'Workloads',
    kinds: [
      { id: 'deployments', label: 'Deployments' },
      { id: 'statefulsets', label: 'StatefulSets' },
      { id: 'daemonsets', label: 'DaemonSets' },
      { id: 'jobs', label: 'Jobs' },
      { id: 'cronjobs', label: 'CronJobs' },
      { id: 'pods', label: 'Pods' },
    ],
  },
  {
    id: 'network',
    label: 'Network',
    kinds: [
      { id: 'services', label: 'Services' },
      { id: 'ingresses', label: 'Ingresses' },
    ],
  },
  {
    id: 'config',
    label: 'Config',
    kinds: [
      { id: 'configmaps', label: 'ConfigMaps' },
      { id: 'secrets', label: 'Secrets' },
    ],
  },
  {
    id: 'storage',
    label: 'Storage',
    kinds: [{ id: 'persistentvolumeclaims', label: 'PVCs' }],
  },
  {
    id: 'cluster',
    label: 'Cluster',
    kinds: [
      { id: 'events', label: 'Events' },
      { id: 'nodes', label: 'Nodes' },
    ],
  },
]

/** @deprecated use RESOURCE_GROUPS */
export const WORKLOAD_KINDS = RESOURCE_GROUPS[0].kinds

export function listNamespaces(clusterId) {
  return api(`/clusters/${clusterId}/k8s/namespaces`)
}

export function listResources(clusterId, kind, namespace) {
  const q =
    namespace && namespace !== 'all'
      ? `?namespace=${encodeURIComponent(namespace)}`
      : ''
  return api(`/clusters/${clusterId}/k8s/resources/${kind}${q}`)
}

export function listWorkloads(clusterId, kind, namespace) {
  return listResources(clusterId, kind, namespace)
}

export function getResource(clusterId, kind, ns, name) {
  const nsPath = encodeURIComponent(ns || '_')
  return api(
    `/clusters/${clusterId}/k8s/resources/${kind}/${nsPath}/${encodeURIComponent(name)}`,
  )
}

export function applyResource(clusterId, kind, ns, name, yaml) {
  const nsPath = encodeURIComponent(ns || '_')
  return api(
    `/clusters/${clusterId}/k8s/resources/${kind}/${nsPath}/${encodeURIComponent(name)}`,
    { method: 'PUT', body: { yaml } },
  )
}

export function applyYaml(clusterId, yaml) {
  return api(`/clusters/${clusterId}/k8s/apply`, {
    method: 'POST',
    body: { yaml },
  })
}

export function deleteResource(clusterId, kind, ns, name) {
  const nsPath = encodeURIComponent(ns || '_')
  return api(
    `/clusters/${clusterId}/k8s/resources/${kind}/${nsPath}/${encodeURIComponent(name)}`,
    { method: 'DELETE' },
  )
}

export function deleteWorkload(clusterId, kind, ns, name) {
  return deleteResource(clusterId, kind, ns, name)
}

export function scaleDeployment(clusterId, ns, name, replicas) {
  return api(`/clusters/${clusterId}/k8s/deployments/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/scale`, {
    method: 'POST',
    body: { replicas },
  })
}

export function restartDeployment(clusterId, ns, name) {
  return api(`/clusters/${clusterId}/k8s/deployments/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/restart`, {
    method: 'POST',
    body: {},
  })
}

export function listHelmReleases(clusterId) {
  return api(`/clusters/${clusterId}/helm/releases`)
}

export function getHelmRelease(clusterId, ns, name) {
  return api(
    `/clusters/${clusterId}/helm/releases/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`,
  )
}

export function helmInstall(clusterId, body) {
  return api(`/clusters/${clusterId}/helm/install`, {
    method: 'POST',
    body,
  })
}

export function helmUninstall(clusterId, ns, name) {
  return api(
    `/clusters/${clusterId}/helm/releases/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`,
    { method: 'DELETE' },
  )
}

export function listAddons(clusterId) {
  return api(`/clusters/${clusterId}/addons`)
}

export function checkAddon(clusterId, name, body) {
  return api(`/clusters/${clusterId}/addons/${encodeURIComponent(name)}/check`, {
    method: 'POST',
    body: body || {},
  })
}

export function installAddon(clusterId, name, body) {
  return api(`/clusters/${clusterId}/addons/${encodeURIComponent(name)}/install`, {
    method: 'POST',
    body: body || {},
  })
}

function wsBase() {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}`
}

function withToken(url) {
  const token = getToken()
  if (token) url.searchParams.set('token', token)
  return url.toString()
}

/** Host OS shell WebSocket (mgmt server) with cluster KUBECONFIG. */
export function buildHostShellWsUrl(clusterId) {
  const url = new URL(`${wsBase()}/api/clusters/${clusterId}/k8s/shell`)
  return withToken(url)
}

/** Management-host shell (pertiskctl / ops tools) — no cluster KUBECONFIG. */
export function buildMgmtShellWsUrl() {
  const url = new URL(`${wsBase()}/api/mgmt/shell`)
  return withToken(url)
}

export function buildPodLogsWsUrl(clusterId, ns, name, { container, follow = true, tail = 200 } = {}) {
  const url = new URL(
    `${wsBase()}/api/clusters/${clusterId}/k8s/pods/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/logs`,
  )
  if (container) url.searchParams.set('container', container)
  url.searchParams.set('follow', follow ? '1' : '0')
  url.searchParams.set('tail', String(tail))
  return withToken(url)
}

export function buildPodExecWsUrl(clusterId, ns, name, { container, shell = 'sh' } = {}) {
  const url = new URL(
    `${wsBase()}/api/clusters/${clusterId}/k8s/pods/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/exec`,
  )
  if (container) url.searchParams.set('container', container)
  if (shell) url.searchParams.set('shell', shell)
  return withToken(url)
}
