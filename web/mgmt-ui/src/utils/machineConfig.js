/** Default Config-tab YAML. `mgmt_url` comes from Settings → Public URL. */
export function defaultMachineConfigYaml(publicUrl) {
  const url = String(publicUrl || '')
    .trim()
    .replace(/\/+$/, '')
  const lines = [
    'version: v1alpha1',
    'machine:',
    '  dashboard:',
    '    theme: catppuccin',
    '    border: bordered',
  ]
  if (url) {
    lines.push(`    mgmt_url: ${url}`)
  }
  return `${lines.join('\n')}\n`
}

export function defaultTemplateYaml(publicUrl) {
  const url = String(publicUrl || '')
    .trim()
    .replace(/\/+$/, '')
  const dash = [
    '  dashboard:',
    '    theme: catppuccin',
    '    border: bordered',
    ...(url ? [`    mgmt_url: ${url}`] : []),
  ].join('\n')
  return `version: v1alpha1
machine:
${dash}
  network:
    hostname: node-1
    interfaces:
      - interface: eth0
        dhcp: true
`
}
