package provider

import (
	"context"
	"fmt"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/datasource/schema"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var _ datasource.DataSource = &providerDataSource{}

type providerDataSource struct {
	client *client.Client
}

type providerDataSourceModel struct {
	ID      types.String `tfsdk:"id"`
	Name    types.String `tfsdk:"name"`
	Kind    types.String `tfsdk:"kind"`
	URL     types.String `tfsdk:"url"`
	Node    types.String `tfsdk:"node"`
	Storage types.String `tfsdk:"storage"`
	Bridge  types.String `tfsdk:"bridge"`
	Arch    types.String `tfsdk:"arch"`
}

func NewProviderDataSource() datasource.DataSource {
	return &providerDataSource{}
}

func (d *providerDataSource) Metadata(_ context.Context, req datasource.MetadataRequest, resp *datasource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_provider"
}

func (d *providerDataSource) Schema(_ context.Context, _ datasource.SchemaRequest, resp *datasource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Look up an existing Pertisk hypervisor provider by name or id.",
		Attributes: map[string]schema.Attribute{
			"id": schema.StringAttribute{
				MarkdownDescription: "Provider UUID. One of id or name is required.",
				Optional:            true,
				Computed:            true,
			},
			"name": schema.StringAttribute{
				MarkdownDescription: "Provider display name. One of id or name is required.",
				Optional:            true,
				Computed:            true,
			},
			"kind": schema.StringAttribute{
				MarkdownDescription: "proxmox | vsphere | nutanix",
				Computed:            true,
			},
			"url": schema.StringAttribute{
				Computed: true,
			},
			"node": schema.StringAttribute{
				Computed: true,
			},
			"storage": schema.StringAttribute{
				Computed: true,
			},
			"bridge": schema.StringAttribute{
				Computed: true,
			},
			"arch": schema.StringAttribute{
				Computed: true,
			},
		},
	}
}

func (d *providerDataSource) Configure(_ context.Context, req datasource.ConfigureRequest, resp *datasource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	c, ok := req.ProviderData.(*client.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected DataSource Configure Type",
			fmt.Sprintf("Expected *client.Client, got: %T", req.ProviderData))
		return
	}
	d.client = c
}

func (d *providerDataSource) Read(ctx context.Context, req datasource.ReadRequest, resp *datasource.ReadResponse) {
	var data providerDataSourceModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &data)...)
	if resp.Diagnostics.HasError() {
		return
	}

	if d.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	id := data.ID.ValueString()
	name := data.Name.ValueString()
	if id == "" && name == "" {
		resp.Diagnostics.AddError("Missing lookup key", "Set id or name")
		return
	}

	var found *client.Provider
	if id != "" {
		p, err := d.client.GetProvider(ctx, id)
		if err != nil {
			resp.Diagnostics.AddError("Get provider failed", err.Error())
			return
		}
		found = p
	} else {
		list, err := d.client.ListProviders(ctx)
		if err != nil {
			resp.Diagnostics.AddError("List providers failed", err.Error())
			return
		}
		for i := range list {
			if list[i].Name == name {
				found = &list[i]
				break
			}
		}
		if found == nil {
			resp.Diagnostics.AddError("Provider not found", fmt.Sprintf("no provider named %q", name))
			return
		}
	}

	data.ID = types.StringValue(found.ID)
	data.Name = types.StringValue(found.Name)
	data.Kind = types.StringValue(found.Kind)
	data.URL = types.StringValue(found.URL)
	data.Node = types.StringValue(found.Node)
	data.Storage = types.StringValue(found.Storage)
	data.Bridge = types.StringValue(found.Bridge)
	data.Arch = types.StringValue(found.Arch)

	resp.Diagnostics.Append(resp.State.Set(ctx, &data)...)
}
