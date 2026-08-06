import { api, getToken } from '../../api'

export const WORKLOAD_KINDS = [
  { id: 'deployments', label: 'Deployments' },
  { id: 'statefulsets', label: 'StatefulSets' },
  { id: 'daemonsets', label: 'DaemonSets' },
  { id: 'jobs', label: 'Jobs' },
  { id: 'cronjobs', label: 'CronJobs' },
  { id: 'pods', label: 'Pods' },
]

export function listNamespaces(clusterId) {
  return api(`/clusters/${clusterId}/k8s/namespaces`)
}

export function listWorkloads(clusterId, kind, namespace) {
  const q =
    namespace && namespace !== 'all'
      ? `?namespace=${encodeURIComponent(namespace)}`
      : ''
  return api(`/clusters/${clusterId}/k8s/workloads/${kind}${q}`)
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

export function deleteWorkload(clusterId, kind, ns, name) {
  return api(
    `/clusters/${clusterId}/k8s/workloads/${kind}/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`,
    { method: 'DELETE' },
  )
}

export function buildExecWsUrl(clusterId, namespace, pod, container) {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  const url = new URL(`${proto}//${window.location.host}/api/clusters/${clusterId}/k8s/exec`)
  url.searchParams.set('namespace', namespace)
  url.searchParams.set('pod', pod)
  if (container) url.searchParams.set('container', container)
  const token = getToken()
  if (token) url.searchParams.set('token', token)
  return url.toString()
}
