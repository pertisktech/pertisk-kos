package provider

import (
	"os"
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/providerserver"
	"github.com/hashicorp/terraform-plugin-go/tfprotov6"
)

// testAccProtoV6ProviderFactories are used to instantiate a provider during
// acceptance testing. The factory function will be invoked for every Terraform
// CLI command executed to create a provider server to which the CLI can
// reattach.
var testAccProtoV6ProviderFactories = map[string]func() (tfprotov6.ProviderServer, error){
	"pertisk": providerserver.NewProtocol6WithError(New("test")()),
}

func testAccPreCheck(t *testing.T) {
	t.Helper()
	if os.Getenv("PERTISK_URL") == "" {
		t.Fatal("PERTISK_URL must be set for acceptance tests")
	}
	if os.Getenv("PERTISK_TOKEN") == "" && (os.Getenv("PERTISK_USERNAME") == "" || os.Getenv("PERTISK_PASSWORD") == "") {
		t.Fatal("set PERTISK_TOKEN or PERTISK_USERNAME+PERTISK_PASSWORD for acceptance tests")
	}
	for _, k := range []string{
		"PERTISK_ACC_PVE_URL",
		"PERTISK_ACC_PVE_TOKEN_ID",
		"PERTISK_ACC_PVE_TOKEN_SECRET",
		"PERTISK_ACC_PVE_NODE",
		"PERTISK_ACC_PVE_STORAGE",
	} {
		if os.Getenv(k) == "" {
			t.Fatalf("%s must be set for acceptance tests", k)
		}
	}
}

func providerConfig() string {
	insecure := "false"
	if os.Getenv("PERTISK_INSECURE") == "1" || os.Getenv("PERTISK_INSECURE") == "true" {
		insecure = "true"
	}
	if tok := os.Getenv("PERTISK_TOKEN"); tok != "" {
		return `
provider "pertisk" {
  url      = "` + os.Getenv("PERTISK_URL") + `"
  token    = "` + tok + `"
  insecure = ` + insecure + `
}
`
	}
	return `
provider "pertisk" {
  url      = "` + os.Getenv("PERTISK_URL") + `"
  username = "` + os.Getenv("PERTISK_USERNAME") + `"
  password = "` + os.Getenv("PERTISK_PASSWORD") + `"
  insecure = ` + insecure + `
}
`
}

func pveProviderConfig(name string) string {
	bridge := os.Getenv("PERTISK_ACC_PVE_BRIDGE")
	if bridge == "" {
		bridge = "vmbr0"
	}
	return `
resource "pertisk_provider" "` + name + `" {
  name         = "` + name + `"
  kind         = "proxmox"
  url          = "` + os.Getenv("PERTISK_ACC_PVE_URL") + `"
  token_id     = "` + os.Getenv("PERTISK_ACC_PVE_TOKEN_ID") + `"
  token_secret = "` + os.Getenv("PERTISK_ACC_PVE_TOKEN_SECRET") + `"
  node         = "` + os.Getenv("PERTISK_ACC_PVE_NODE") + `"
  storage      = "` + os.Getenv("PERTISK_ACC_PVE_STORAGE") + `"
  bridge       = "` + bridge + `"
  insecure     = true
}
`
}
