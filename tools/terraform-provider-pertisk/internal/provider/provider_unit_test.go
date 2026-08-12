package provider

import (
	"context"
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/resource"
)

func TestProviderMetadata(t *testing.T) {
	p := New("test")()
	var resp provider.MetadataResponse
	p.Metadata(context.Background(), provider.MetadataRequest{}, &resp)
	if resp.TypeName != "pertisk" {
		t.Fatalf("TypeName = %q, want pertisk", resp.TypeName)
	}
	if resp.Version != "test" {
		t.Fatalf("Version = %q, want test", resp.Version)
	}
}

func TestProviderSchemaHasAuth(t *testing.T) {
	p := New("test")()
	var resp provider.SchemaResponse
	p.Schema(context.Background(), provider.SchemaRequest{}, &resp)
	if resp.Diagnostics.HasError() {
		t.Fatalf("schema diagnostics: %v", resp.Diagnostics)
	}
	for _, key := range []string{"url", "username", "password", "token", "insecure"} {
		if _, ok := resp.Schema.Attributes[key]; !ok {
			t.Fatalf("missing provider attribute %q", key)
		}
	}
}

func TestClusterSchemaHasSizing(t *testing.T) {
	r := NewClusterResource()
	var resp resource.SchemaResponse
	r.Schema(context.Background(), resource.SchemaRequest{}, &resp)
	if resp.Diagnostics.HasError() {
		t.Fatalf("schema diagnostics: %v", resp.Diagnostics)
	}
	for _, key := range []string{
		"name", "provider_id", "controlplanes", "workers",
		"cp_memory", "cp_cores", "cp_disk_gb",
		"worker_memory", "worker_cores", "worker_disk_gb",
		"vip", "network_mode", "status", "endpoint", "kubeconfig",
	} {
		if _, ok := resp.Schema.Attributes[key]; !ok {
			t.Fatalf("missing cluster attribute %q", key)
		}
	}
}

func TestClusterResourceTypeName(t *testing.T) {
	r := NewClusterResource()
	var resp resource.MetadataResponse
	r.Metadata(context.Background(), resource.MetadataRequest{ProviderTypeName: "pertisk"}, &resp)
	if resp.TypeName != "pertisk_cluster" {
		t.Fatalf("TypeName = %q, want pertisk_cluster", resp.TypeName)
	}
}
