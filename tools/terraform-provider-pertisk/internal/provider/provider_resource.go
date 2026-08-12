package provider

import (
	"context"
	"fmt"
	"net/http"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/booldefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/boolplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringdefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var (
	_ resource.Resource                = &hypervisorProviderResource{}
	_ resource.ResourceWithImportState = &hypervisorProviderResource{}
)

type hypervisorProviderResource struct {
	client *client.Client
}

type hypervisorProviderModel struct {
	ID          types.String `tfsdk:"id"`
	Name        types.String `tfsdk:"name"`
	Kind        types.String `tfsdk:"kind"`
	URL         types.String `tfsdk:"url"`
	TokenID     types.String `tfsdk:"token_id"`
	TokenSecret types.String `tfsdk:"token_secret"`
	Node        types.String `tfsdk:"node"`
	Storage     types.String `tfsdk:"storage"`
	Bridge      types.String `tfsdk:"bridge"`
	Insecure    types.Bool   `tfsdk:"insecure"`
	Arch        types.String `tfsdk:"arch"`
}

func NewProviderResource() resource.Resource {
	return &hypervisorProviderResource{}
}

func (r *hypervisorProviderResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_provider"
}

func (r *hypervisorProviderResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Register a Proxmox or vSphere (ESXi) hypervisor in pertisk-mgmt. Create probes the hypervisor before saving.",
		Attributes: map[string]schema.Attribute{
			"id": schema.StringAttribute{
				Computed: true,
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"name": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Display name in mgmt UI.",
			},
			"kind": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				Default:             stringdefault.StaticString("proxmox"),
				MarkdownDescription: "proxmox | vsphere | nutanix",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"url": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Hypervisor API URL (e.g. https://pve:8006 or https://esxi).",
			},
			"token_id": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Proxmox token id (user@realm!token) or ESXi username.",
			},
			"token_secret": schema.StringAttribute{
				Required:            true,
				Sensitive:           true,
				MarkdownDescription: "Proxmox token secret or ESXi password. Never returned by API; kept in Terraform state.",
			},
			"node": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Proxmox node name or ESXi host inventory name.",
			},
			"storage": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Proxmox storage or ESXi datastore.",
			},
			"bridge": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				Default:             stringdefault.StaticString("vmbr0"),
				MarkdownDescription: "Proxmox bridge or ESXi portgroup (e.g. VM Network).",
			},
			"insecure": schema.BoolAttribute{
				Optional:            true,
				Computed:            true,
				Default:             booldefault.StaticBool(false),
				MarkdownDescription: "Skip TLS verify for self-signed hypervisor certs.",
				PlanModifiers: []planmodifier.Bool{
					boolplanmodifier.UseStateForUnknown(),
				},
			},
			"arch": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "Default guest arch: amd64 | arm64. Omit for auto-detect from hypervisor probe.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
		},
	}
}

func (r *hypervisorProviderResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	c, ok := req.ProviderData.(*client.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected Resource Configure Type",
			fmt.Sprintf("Expected *client.Client, got: %T", req.ProviderData))
		return
	}
	r.client = c
}

func (r *hypervisorProviderResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan hypervisorProviderModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	write := client.ProviderWriteRequest{
		Name:        plan.Name.ValueString(),
		URL:         plan.URL.ValueString(),
		TokenID:     plan.TokenID.ValueString(),
		TokenSecret: plan.TokenSecret.ValueString(),
		Node:        plan.Node.ValueString(),
		Storage:     plan.Storage.ValueString(),
		Bridge:      plan.Bridge.ValueString(),
		Insecure:    plan.Insecure.ValueBool(),
		Kind:        plan.Kind.ValueString(),
	}
	if !plan.Arch.IsNull() && !plan.Arch.IsUnknown() && plan.Arch.ValueString() != "" {
		write.Arch = plan.Arch.ValueString()
	}

	created, err := r.client.CreateProvider(ctx, write)
	if err != nil {
		resp.Diagnostics.AddError("Create provider failed", err.Error())
		return
	}

	secret := plan.TokenSecret
	r.flatten(created, &plan)
	plan.TokenSecret = secret
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *hypervisorProviderResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state hypervisorProviderModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	p, err := r.client.GetProvider(ctx, state.ID.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			resp.State.RemoveResource(ctx)
			return
		}
		resp.Diagnostics.AddError("Read provider failed", err.Error())
		return
	}

	secret := state.TokenSecret
	r.flatten(p, &state)
	state.TokenSecret = secret
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *hypervisorProviderResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan hypervisorProviderModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	var state hypervisorProviderModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	name := plan.Name.ValueString()
	url := plan.URL.ValueString()
	tokenID := plan.TokenID.ValueString()
	node := plan.Node.ValueString()
	storage := plan.Storage.ValueString()
	bridge := plan.Bridge.ValueString()
	insecure := plan.Insecure.ValueBool()
	arch := plan.Arch.ValueString()
	secret := plan.TokenSecret.ValueString()

	patch := client.ProviderPatchRequest{
		Name:     &name,
		URL:      &url,
		TokenID:  &tokenID,
		Node:     &node,
		Storage:  &storage,
		Bridge:   &bridge,
		Insecure: &insecure,
	}
	if !plan.Arch.IsNull() && !plan.Arch.IsUnknown() && arch != "" {
		patch.Arch = &arch
	}
	if secret != "" {
		patch.TokenSecret = &secret
	}

	updated, err := r.client.UpdateProvider(ctx, state.ID.ValueString(), patch)
	if err != nil {
		resp.Diagnostics.AddError("Update provider failed", err.Error())
		return
	}

	kept := plan.TokenSecret
	r.flatten(updated, &plan)
	plan.TokenSecret = kept
	plan.ID = state.ID
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *hypervisorProviderResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state hypervisorProviderModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	if err := r.client.DeleteProvider(ctx, state.ID.ValueString()); err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			return
		}
		resp.Diagnostics.AddError("Delete provider failed",
			err.Error()+" (mgmt requires admin role to delete providers)")
		return
	}
}

func (r *hypervisorProviderResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("id"), req, resp)
}

func (r *hypervisorProviderResource) flatten(p *client.Provider, m *hypervisorProviderModel) {
	m.ID = types.StringValue(p.ID)
	m.Name = types.StringValue(p.Name)
	m.Kind = types.StringValue(p.Kind)
	m.URL = types.StringValue(p.URL)
	m.TokenID = types.StringValue(p.TokenID)
	m.Node = types.StringValue(p.Node)
	m.Storage = types.StringValue(p.Storage)
	m.Bridge = types.StringValue(p.Bridge)
	m.Insecure = types.BoolValue(p.Insecure != 0)
	m.Arch = types.StringValue(p.Arch)
}
