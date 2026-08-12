package provider

import (
	"fmt"
	"os"
	"testing"
	"time"

	"github.com/hashicorp/terraform-plugin-testing/helper/resource"
)

// TestAccPertiskCluster_threeNode creates a minimal 3-node cluster on Proxmox
// (1 control-plane + 2 workers) with explicit CPU/memory/disk sizing.
//
// Required env:
//
//	TF_ACC=1
//	PERTISK_URL, PERTISK_USERNAME+PERTISK_PASSWORD (or PERTISK_TOKEN), optional PERTISK_INSECURE=1
//	PERTISK_ACC_PVE_URL, PERTISK_ACC_PVE_TOKEN_ID, PERTISK_ACC_PVE_TOKEN_SECRET
//	PERTISK_ACC_PVE_NODE, PERTISK_ACC_PVE_STORAGE, optional PERTISK_ACC_PVE_BRIDGE / PERTISK_ACC_CP_VMID
func TestAccPertiskCluster_threeNode(t *testing.T) {
	if os.Getenv("TF_ACC") == "" {
		t.Skip("Acceptance tests skipped unless TF_ACC=1")
	}
	testAccPreCheck(t)

	suffix := fmt.Sprintf("%d", time.Now().Unix()%100000)
	clusterName := "tf-acc-" + suffix
	providerName := "tf-acc-pve-" + suffix
	vmid := "410"
	if v := os.Getenv("PERTISK_ACC_CP_VMID"); v != "" {
		vmid = v
	}

	resource.Test(t, resource.TestCase{
		ProtoV6ProviderFactories: testAccProtoV6ProviderFactories,
		Steps: []resource.TestStep{
			{
				Config: providerConfig() + pveProviderConfig(providerName) + fmt.Sprintf(`
resource "pertisk_cluster" "lab" {
  name            = %q
  provider_id     = pertisk_provider.%s.id
  controlplanes   = 1
  workers         = 2
  cni             = "cilium"
  network_mode    = "ipv4"
  cp_vmid         = %s
  k8s_version     = "v1.36.3"
  timeout_minutes = 60

  cp_memory      = 4096
  cp_cores       = 2
  cp_disk_gb     = 50
  worker_memory  = 4096
  worker_cores   = 2
  worker_disk_gb = 50
}
`, clusterName, providerName, vmid),
				Check: resource.ComposeAggregateTestCheckFunc(
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "name", clusterName),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "controlplanes", "1"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "workers", "2"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "cp_memory", "4096"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "cp_cores", "2"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "cp_disk_gb", "50"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "worker_memory", "4096"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "worker_cores", "2"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "worker_disk_gb", "50"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "cni", "cilium"),
					resource.TestCheckResourceAttr("pertisk_cluster.lab", "status", "ready"),
					resource.TestCheckResourceAttrSet("pertisk_cluster.lab", "id"),
					resource.TestCheckResourceAttrSet("pertisk_cluster.lab", "endpoint"),
					resource.TestCheckResourceAttrSet("pertisk_cluster.lab", "kubeconfig"),
				),
			},
		},
	})
}
