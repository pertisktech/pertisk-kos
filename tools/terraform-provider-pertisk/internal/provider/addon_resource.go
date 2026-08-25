package provider

import (
	"context"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/int64default"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/hashicorp/terraform-plugin-log/tflog"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var (
	_ resource.Resource                = &addonResource{}
	_ resource.ResourceWithImportState = &addonResource{}
)

type addonResource struct {
	client *client.Client
}

type addonModel struct {
	ID             types.String `tfsdk:"id"`
	ClusterID      types.String `tfsdk:"cluster_id"`
	Addon          types.String `tfsdk:"addon"`
	Config         types.Map    `tfsdk:"config"`
	Secrets        types.Map    `tfsdk:"secrets"`
	Status         types.String `tfsdk:"status"`
	TimeoutMinutes types.Int64  `tfsdk:"timeout_minutes"`
}

func NewAddonResource() resource.Resource {
	return &addonResource{}
}

func (r *addonResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_addon"
}

func (r *addonResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Install or update a cluster add-on via pertisk-mgmt (`nfs`, `cert-manager`, `cilium-lb`, `ingress`). Destroy only drops Terraform state; the add-on stays on the cluster (mgmt has no uninstall API).",
		Attributes: map[string]schema.Attribute{
			"id": schema.StringAttribute{
				Computed:            true,
				MarkdownDescription: "`cluster_id/addon`.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"cluster_id": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Cluster UUID. Forces new resource.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"addon": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Catalog id: `nfs` | `cert-manager` | `cilium-lb` | `ingress`. Forces new resource.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"config": schema.MapAttribute{
				ElementType:         types.StringType,
				Optional:            true,
				MarkdownDescription: "Non-secret fields (NFS server/path, cert-manager email/domain, Cilium LB IPs, ingress image_tag/admin_host, …).",
			},
			"secrets": schema.MapAttribute{
				ElementType:         types.StringType,
				Optional:            true,
				Sensitive:           true,
				MarkdownDescription: "Secret fields: cert-manager `api_token`; ingress `admin_password`, `registry_password`.",
			},
			"status": schema.StringAttribute{
				Computed:            true,
				MarkdownDescription: "Addon status from mgmt (`installed`, `installing`, `error`, …).",
			},
			"timeout_minutes": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(20),
			},
		},
	}
}

func (r *addonResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
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

func (r *addonResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan addonModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.installAndWait(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Install addon failed", err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *addonResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state addonModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	sum, err := r.client.GetAddon(ctx, state.ClusterID.ValueString(), state.Addon.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			resp.State.RemoveResource(ctx)
			return
		}
		resp.Diagnostics.AddError("Read addon failed", err.Error())
		return
	}
	state.Status = types.StringValue(sum.Status)
	if state.TimeoutMinutes.IsNull() || state.TimeoutMinutes.IsUnknown() {
		state.TimeoutMinutes = types.Int64Value(20)
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *addonResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan addonModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.installAndWait(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Update addon failed", err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *addonResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state addonModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	tflog.Warn(ctx, "pertisk_addon destroy does not uninstall the add-on on the cluster", map[string]any{
		"cluster_id": state.ClusterID.ValueString(),
		"addon":      state.Addon.ValueString(),
	})
}

func (r *addonResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	parts := strings.SplitN(req.ID, "/", 2)
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		resp.Diagnostics.AddError("Invalid import id", "Use cluster_id/addon (e.g. uuid/nfs).")
		return
	}
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("id"), req.ID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("cluster_id"), parts[0])...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("addon"), parts[1])...)
}

func (r *addonResource) installAndWait(ctx context.Context, plan *addonModel) error {
	if r.client == nil {
		return fmt.Errorf("provider client is nil")
	}
	addon := plan.Addon.ValueString()
	switch addon {
	case "nfs", "cert-manager", "cilium-lb", "ingress":
	default:
		return fmt.Errorf("addon must be nfs, cert-manager, cilium-lb, or ingress")
	}

	body, err := addonInstallBody(plan)
	if err != nil {
		return err
	}
	timeout := plan.TimeoutMinutes.ValueInt64()
	if timeout <= 0 {
		timeout = 20
	}
	waitCtx, cancel := context.WithTimeout(ctx, time.Duration(timeout)*time.Minute)
	defer cancel()

	out, err := r.client.InstallAddon(waitCtx, plan.ClusterID.ValueString(), addon, body)
	if err != nil {
		return err
	}
	tflog.Info(ctx, "addon install enqueued", map[string]any{"addon": addon, "job_id": out.JobID})
	if out.JobID != "" {
		if _, err := r.client.WaitJob(waitCtx, out.JobID, 5*time.Second); err != nil {
			return err
		}
	}

	sum, err := waitAddonInstalled(waitCtx, r.client, plan.ClusterID.ValueString(), addon)
	if err != nil {
		return err
	}
	plan.ID = types.StringValue(plan.ClusterID.ValueString() + "/" + addon)
	plan.Status = types.StringValue(sum.Status)
	if plan.TimeoutMinutes.IsNull() || plan.TimeoutMinutes.IsUnknown() {
		plan.TimeoutMinutes = types.Int64Value(20)
	}
	if plan.Config.IsNull() || plan.Config.IsUnknown() {
		plan.Config = types.MapNull(types.StringType)
	}
	if plan.Secrets.IsNull() || plan.Secrets.IsUnknown() {
		plan.Secrets = types.MapNull(types.StringType)
	}
	return nil
}

func addonInstallBody(plan *addonModel) (map[string]any, error) {
	body := map[string]any{}
	if err := mergeStringMap(plan.Config, body); err != nil {
		return nil, err
	}
	if err := mergeStringMap(plan.Secrets, body); err != nil {
		return nil, err
	}
	return body, nil
}

func mergeStringMap(m types.Map, into map[string]any) error {
	if m.IsNull() || m.IsUnknown() {
		return nil
	}
	var raw map[string]string
	if diags := m.ElementsAs(context.Background(), &raw, false); diags.HasError() {
		return fmt.Errorf("invalid map: %s", diags.Errors())
	}
	for k, v := range raw {
		into[k] = v
	}
	return nil
}

func waitAddonInstalled(ctx context.Context, c *client.Client, clusterID, addon string) (*client.AddonSummary, error) {
	ticker := time.NewTicker(3 * time.Second)
	defer ticker.Stop()
	for {
		sum, err := c.GetAddon(ctx, clusterID, addon)
		if err != nil {
			return nil, err
		}
		switch sum.Status {
		case "installed":
			return sum, nil
		case "error":
			msg := "addon install error"
			if sum.Error != nil && *sum.Error != "" {
				msg = *sum.Error
			}
			return sum, fmt.Errorf("%s", msg)
		}
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("timeout waiting for addon %s (last status %s): %w", addon, sum.Status, ctx.Err())
		case <-ticker.C:
		}
	}
}
