/** JSON Schema for Pertisk machine YAML (`version: v1alpha1`). */
export const machineConfigSchema = {
  $schema: 'http://json-schema.org/draft-07/schema#',
  $id: 'inmemory://schema/pertisk-machine-config.json',
  title: 'Pertisk machine config',
  description: 'Machine configuration applied by pertiskctl (v1alpha1). Partial updates merge with each node’s on-disk config.',
  type: 'object',
  required: ['version', 'machine'],
  additionalProperties: true,
  properties: {
    version: {
      type: 'string',
      const: 'v1alpha1',
      description: 'Config schema version. Must be v1alpha1.',
    },
    machine: {
      type: 'object',
      additionalProperties: true,
      description: 'Node OS settings. machine.type is set per node role on cluster apply.',
      properties: {
        type: {
          type: 'string',
          enum: ['controlplane', 'worker'],
          description: 'Node role. Cluster Config apply overwrites this from the node’s role so workers are not flipped to controlplane.',
        },
        network: {
          type: 'object',
          additionalProperties: true,
          description: 'Hostname, interfaces, and DNS.',
          properties: {
            hostname: {
              type: 'string',
              description: 'Node hostname.',
            },
            interfaces: {
              type: 'array',
              description: 'L3 interfaces.',
              items: {
                type: 'object',
                additionalProperties: true,
                required: ['interface'],
                properties: {
                  interface: {
                    type: 'string',
                    description: 'Kernel interface name (eth0, ens192, …).',
                  },
                  dhcp: {
                    type: 'boolean',
                    description: 'Use DHCP for this interface.',
                  },
                  addresses: {
                    type: 'array',
                    items: { type: 'string' },
                    description: 'CIDR addresses when dhcp is false (e.g. 10.0.0.5/24).',
                  },
                  gateway: {
                    type: 'string',
                    description: 'Default gateway when not using DHCP.',
                  },
                },
              },
            },
            nameservers: {
              type: 'array',
              items: { type: 'string' },
              description: 'DNS nameservers when not assigned by DHCP.',
            },
          },
        },
        install: {
          type: 'object',
          additionalProperties: true,
          description: 'Bare-metal / disk install target.',
          properties: {
            disk: {
              type: 'string',
              description: 'Block device to install onto (e.g. /dev/vda, /dev/sda).',
            },
            wipe: {
              type: 'boolean',
              description: 'Wipe existing partition table before installing.',
            },
          },
        },
        dashboard: {
          type: 'object',
          additionalProperties: true,
          description: 'Serial / xterm.js status dashboard. Omit for built-in defaults.',
          properties: {
            theme: {
              type: 'string',
              enum: [
                'dracula',
                'nord',
                'gruvbox',
                'wild-cherry',
                'tokyo-night',
                'catppuccin',
                'solarized',
                'cyberpunk',
                'mono',
              ],
              description: 'Console color theme. Default: catppuccin.',
            },
            border: {
              type: 'string',
              enum: ['auto', 'ascii', 'light', 'rounded', 'heavy', 'double', 'bordered', 'line'],
              description: 'Frame style. Default: line (ASCII = frames, Serial-safe).',
            },
            background: {
              type: 'string',
              description: 'Dashboard background as #RRGGBB. Omit for the terminal default.',
            },
            cols: {
              type: 'integer',
              minimum: 1,
              description: 'Force column count (skips size probe). Wrong size blanks Serial.',
            },
            rows: {
              type: 'integer',
              minimum: 1,
              description: 'Force row count (skips size probe).',
            },
            utf8: {
              type: 'boolean',
              description: 'Force Unicode box-drawing. Set false if Serial mangles multi-byte glyphs.',
            },
            mgmt_url: {
              type: 'string',
              description: 'Public web management URL shown on the serial console. Also set via Settings → Public URL.',
            },
            mgmtUrl: {
              type: 'string',
              description: 'Alias for mgmt_url.',
            },
          },
        },
        kubelet: {
          type: 'object',
          additionalProperties: true,
          description: 'Kubelet settings written to /var/lib/kubelet/config.yaml.',
          properties: {
            extraConfig: {
              type: 'object',
              additionalProperties: true,
              description: 'Extra KubeletConfiguration fields merged into the written config.',
              properties: {
                maxPods: {
                  type: 'integer',
                  minimum: 1,
                  description: 'Max pods per node. Upstream kubelet default is 110 when omitted.',
                },
              },
            },
          },
        },
        observability: {
          type: 'object',
          additionalProperties: true,
          description: 'Optional log/metrics ship. Omit or empty lokiUrl disables the pusher.',
          properties: {
            lokiUrl: {
              type: 'string',
              description: 'Loki push URL, e.g. http://10.1.1.10:3500/loki/api/v1/push.',
            },
            lokiToken: {
              type: 'string',
              description: 'Optional Authorization: Bearer for the Loki push endpoint.',
            },
            prometheusPushUrl: {
              type: 'string',
              description: 'Prometheus Pushgateway base URL. When omitted, derived from lokiUrl if that uses Alloy port 3500.',
            },
            extraLabels: {
              type: 'object',
              additionalProperties: { type: 'string' },
              description: 'Extra stream labels (merged after job / service / hostname / cluster).',
            },
          },
        },
      },
    },
    cluster: {
      type: 'object',
      additionalProperties: true,
      description: 'Kubernetes cluster join / bootstrap. Preserved on partial Config-tab apply.',
      properties: {
        name: {
          type: 'string',
          description: 'Logical cluster name (kubeconfig context / cluster entry).',
        },
        endpoint: {
          type: 'string',
          description: 'API server URL, e.g. https://10.1.1.210:6443.',
        },
        token: {
          type: 'string',
          description: 'Bootstrap / join token.',
        },
        ca: {
          type: 'string',
          description: 'PEM-encoded cluster CA certificate.',
        },
        caKey: {
          type: 'string',
          description: 'PEM-encoded cluster CA private key (control-plane bootstrap only).',
        },
        saKey: {
          type: 'string',
          description: 'PEM-encoded service-account signing key (control-plane bootstrap only).',
        },
        network: {
          type: 'object',
          additionalProperties: true,
          description: 'Talos-style pod/service subnet lists (preferred for new configs).',
          properties: {
            podSubnets: {
              type: 'array',
              items: { type: 'string' },
              description: 'Pod CIDRs (IPv4 and optional IPv6).',
            },
            serviceSubnets: {
              type: 'array',
              items: { type: 'string' },
              description: 'Service CIDRs (IPv4 and optional IPv6).',
            },
          },
        },
        podSubnet: {
          type: 'string',
          description: 'Legacy cluster-wide pod network CIDR (e.g. 10.244.0.0/16).',
        },
        serviceSubnet: {
          type: 'string',
          description: 'Legacy cluster service CIDR (default 10.96.0.0/12).',
        },
        podCidrIPv6: {
          type: 'string',
          description: 'Legacy IPv6 pod CIDR when networkMode is dual-stack.',
        },
        serviceCidrIPv6: {
          type: 'string',
          description: 'Legacy IPv6 service CIDR when dual-stack.',
        },
        networkMode: {
          type: 'string',
          enum: ['ipv4', 'dual-stack'],
          description: 'Node / cluster IP family mode. Default ipv4.',
        },
        vip6: {
          type: 'string',
          description: 'Optional IPv6 API VIP (HA dual-stack); also added to cert SANs.',
        },
        kubernetesVersion: {
          type: 'string',
          description: 'Kubernetes version tag for static-pod images (e.g. v1.32.5).',
        },
        podCidr: {
          type: 'string',
          description: 'Pod CIDR for this node’s bridge CNI. Unused when cni: none.',
        },
        cni: {
          type: 'string',
          enum: ['bridge', 'none'],
          description: 'Pod networking: bridge (built-in) or none (Flannel/Cilium/etc.).',
        },
        certSANs: {
          type: 'array',
          items: { type: 'string' },
          description: 'Extra apiserver (and etcd) certificate SANs — VIP, extra DNS names, CP IPs.',
        },
      },
    },
  },
}
