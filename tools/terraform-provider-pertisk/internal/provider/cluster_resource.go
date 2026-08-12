package provider

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/int64default"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/int64planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringdefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/hashicorp/terraform-plugin-log/tflog"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var (
	_ resource.Resource                = &clusterResource{}
	_ resource.ResourceWithImportState = &clusterResource{}
	_ resource.ResourceWithModifyPlan  = &clusterResource{}
)

type clusterResource struct {
	client *client.Client
}

type clusterModel struct {
	ID                types.String `tfsdk:"id"`
	Name              types.String `tfsdk:"name"`
	ProviderID        types.String `tfsdk:"provider_id"`
	Controlplanes     types.Int64  `tfsdk:"controlplanes"`
	Workers           types.Int64  `tfsdk:"workers"`
	NetworkMode       types.String `tfsdk:"network_mode"`
	VIP               types.String `tfsdk:"vip"`
	VIP6              types.String `tfsdk:"vip6"`
	CNI               types.String `tfsdk:"cni"`
	K8sVersion        types.String `tfsdk:"k8s_version"`
	CPMemory          types.Int64  `tfsdk:"cp_memory"`
	CPCores           types.Int64  `tfsdk:"cp_cores"`
	CPDiskGB          types.Int64  `tfsdk:"cp_disk_gb"`
	WorkerMemory      types.Int64  `tfsdk:"worker_memory"`
	WorkerCores       types.Int64  `tfsdk:"worker_cores"`
	WorkerDiskGB      types.Int64  `tfsdk:"worker_disk_gb"`
	CPVMID            types.Int64  `tfsdk:"cp_vmid"`
	MaxPods           types.Int64  `tfsdk:"max_pods"`
	Arch              types.String `tfsdk:"arch"`
	PodSubnet         types.String `tfsdk:"pod_subnet"`
	ServiceSubnet     types.String `tfsdk:"service_subnet"`
	PodSubnetIPv6     types.String `tfsdk:"pod_subnet_ipv6"`
	ServiceSubnetIPv6 types.String `tfsdk:"service_subnet_ipv6"`
	Status            types.String `tfsdk:"status"`
	Endpoint          types.String `tfsdk:"endpoint"`
	Kubeconfig        types.String `tfsdk:"kubeconfig"`
	TimeoutMinutes    types.Int64  `tfsdk:"timeout_minutes"`
}

func NewClusterResource() resource.Resource {
	return &clusterResource{}
}

func (r *clusterResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_cluster"
}

func (r *clusterResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Create a Pertisk cluster on an existing Proxmox/vSphere provider via pertisk-mgmt.",
		Attributes: map[string]schema.Attribute{
			"id": schema.StringAttribute{
				Computed:            true,
				MarkdownDescription: "Cluster UUID.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"name": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Cluster name (VM prefix: {name}-cp-N / {name}-wk-N).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"provider_id": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "Existing mgmt provider UUID (Proxmox or vSphere).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"controlplanes": schema.Int64Attribute{
				Optional:            true,
				Computed:            true,
				Default:             int64default.StaticInt64(1),
				MarkdownDescription: "Initial control-plane count at create only. Live inventory is not synced; scale with pertisk_node.",
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.UseStateForUnknown(),
				},
			},
			"workers": schema.Int64Attribute{
				Optional:            true,
				Computed:            true,
				Default:             int64default.StaticInt64(1),
				MarkdownDescription: "Initial worker count at create only. Live inventory is not synced; scale with pertisk_node.",
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.UseStateForUnknown(),
				},
			},
			"network_mode": schema.StringAttribute{
				Optional: true,
				Computed: true,
				Default:  stringdefault.StaticString("ipv4"),
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"vip": schema.StringAttribute{
				Optional:            true,
				MarkdownDescription: "IPv4 VIP (kube-vip). Required when controlplanes > 1 and network_mode is ipv4/dual-stack.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"vip6": schema.StringAttribute{
				Optional: true,
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"cni": schema.StringAttribute{
				Optional: true,
				Computed: true,
				Default:  stringdefault.StaticString("cilium"),
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"k8s_version": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				Default:             stringdefault.StaticString("v1.36.3"),
				MarkdownDescription: "Kubernetes version. Changing this triggers an in-place upgrade job.",
			},
			"cp_memory": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(4096),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"cp_cores": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(2),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"cp_disk_gb": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(50),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"worker_memory": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(8192),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"worker_cores": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(4),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"worker_disk_gb": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(75),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"cp_vmid": schema.Int64Attribute{
				Optional:            true,
				Computed:            true,
				Default:             int64default.StaticInt64(210),
				MarkdownDescription: "Base VMID (inventory). First CP uses this, then +1…",
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"max_pods": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(250),
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"arch": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "Guest arch: amd64|arm64. Omit to use provider default.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"pod_subnet": schema.StringAttribute{
				Optional: true,
				Computed: true,
				Default:  stringdefault.StaticString("10.244.0.0/16"),
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"service_subnet": schema.StringAttribute{
				Optional: true,
				Computed: true,
				Default:  stringdefault.StaticString("10.96.0.0/12"),
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"pod_subnet_ipv6": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "IPv6 pod CIDR. When omitted on dual-stack, mgmt applies its default (e.g. 2001:db8:10:0::/56).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"service_subnet_ipv6": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "IPv6 service CIDR. When omitted on dual-stack, mgmt applies its default (e.g. 2001:db8:96:1::/112).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"status": schema.StringAttribute{
				Computed:            true,
				MarkdownDescription: "Cluster status from mgmt (ready, pending, error, …).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"endpoint": schema.StringAttribute{
				Computed:            true,
				MarkdownDescription: "API server endpoint when available.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"kubeconfig": schema.StringAttribute{
				Computed:            true,
				Sensitive:           true,
				MarkdownDescription: "Admin kubeconfig YAML from mgmt (empty until cluster is ready).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"timeout_minutes": schema.Int64Attribute{
				Optional:            true,
				Computed:            true,
				Default:             int64default.StaticInt64(45),
				MarkdownDescription: "How long to wait for create/delete jobs.",
			},
		},
	}
}

func (r *clusterResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
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

func (r *clusterResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan clusterModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	apiReq := client.CreateClusterRequest{
		Name:          plan.Name.ValueString(),
		ProviderID:    plan.ProviderID.ValueString(),
		Controlplanes: plan.Controlplanes.ValueInt64(),
		Workers:       plan.Workers.ValueInt64(),
		NetworkMode:   plan.NetworkMode.ValueString(),
		CNI:           plan.CNI.ValueString(),
		K8sVersion:    plan.K8sVersion.ValueString(),
		CPMemory:      plan.CPMemory.ValueInt64(),
		CPCores:       plan.CPCores.ValueInt64(),
		CPDiskGB:      plan.CPDiskGB.ValueInt64(),
		WorkerMemory:  plan.WorkerMemory.ValueInt64(),
		WorkerCores:   plan.WorkerCores.ValueInt64(),
		WorkerDiskGB:  plan.WorkerDiskGB.ValueInt64(),
		CPVMID:        plan.CPVMID.ValueInt64(),
		MaxPods:       plan.MaxPods.ValueInt64(),
		PodSubnet:     plan.PodSubnet.ValueString(),
		ServiceSubnet: plan.ServiceSubnet.ValueString(),
	}
	if !plan.VIP.IsNull() && !plan.VIP.IsUnknown() && plan.VIP.ValueString() != "" {
		v := plan.VIP.ValueString()
		apiReq.VIP = &v
	}
	if !plan.VIP6.IsNull() && !plan.VIP6.IsUnknown() && plan.VIP6.ValueString() != "" {
		v := plan.VIP6.ValueString()
		apiReq.VIP6 = &v
	}
	if !plan.Arch.IsNull() && !plan.Arch.IsUnknown() && plan.Arch.ValueString() != "" {
		v := plan.Arch.ValueString()
		apiReq.Arch = &v
	}
	if !plan.PodSubnetIPv6.IsNull() && !plan.PodSubnetIPv6.IsUnknown() && plan.PodSubnetIPv6.ValueString() != "" {
		v := plan.PodSubnetIPv6.ValueString()
		apiReq.PodSubnetIPv6 = &v
	}
	if !plan.ServiceSubnetIPv6.IsNull() && !plan.ServiceSubnetIPv6.IsUnknown() && plan.ServiceSubnetIPv6.ValueString() != "" {
		v := plan.ServiceSubnetIPv6.ValueString()
		apiReq.ServiceSubnetIPv6 = &v
	}

	created, err := r.client.CreateCluster(ctx, apiReq)
	if err != nil {
		resp.Diagnostics.AddError("Create cluster failed", err.Error())
		return
	}
	tflog.Info(ctx, "cluster create enqueued", map[string]any{"id": created.ID, "job_id": created.JobID})

	waitCtx, cancel := context.WithTimeout(ctx, time.Duration(plan.TimeoutMinutes.ValueInt64())*time.Minute)
	defer cancel()

	if created.JobID != "" {
		if _, err := r.client.WaitJob(waitCtx, created.JobID, 5*time.Second); err != nil {
			r.savePartialCreate(ctx, &plan, created.ID, resp)
			resp.Diagnostics.AddError("Create cluster job failed", err.Error())
			return
		}
	}

	cl, err := r.client.GetCluster(waitCtx, created.ID)
	if err != nil {
		r.savePartialCreate(ctx, &plan, created.ID, resp)
		resp.Diagnostics.AddError("Read cluster after create failed", err.Error())
		return
	}
	if cl.Status == "error" {
		msg := cl.Status
		if cl.Error != nil {
			msg = *cl.Error
		}
		r.flatten(cl, &plan)
		r.fetchKubeconfig(ctx, &plan)
		r.ensureComputedKnown(&plan)
		resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
		resp.Diagnostics.AddError("Cluster ended in error", msg)
		return
	}

	r.flatten(cl, &plan)
	r.fetchKubeconfig(ctx, &plan)
	r.ensureComputedKnown(&plan)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

// savePartialCreate persists a known object after a failed create so terraform
// destroy can clean up, without leaving Unknown computed attributes.
func (r *clusterResource) savePartialCreate(ctx context.Context, plan *clusterModel, id string, resp *resource.CreateResponse) {
	plan.ID = types.StringValue(id)
	if cl, err := r.client.GetCluster(ctx, id); err == nil {
		r.flatten(cl, plan)
		r.fetchKubeconfig(ctx, plan)
	} else {
		plan.Status = types.StringValue("error")
		plan.Endpoint = types.StringNull()
		plan.Kubeconfig = types.StringNull()
		if plan.Arch.IsNull() || plan.Arch.IsUnknown() {
			plan.Arch = types.StringNull()
		}
	}
	r.ensureComputedKnown(plan)
	resp.Diagnostics.Append(resp.State.Set(ctx, plan)...)
}

func (r *clusterResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state clusterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	cl, err := r.client.GetCluster(ctx, state.ID.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			resp.State.RemoveResource(ctx)
			return
		}
		resp.Diagnostics.AddError("Read cluster failed", err.Error())
		return
	}
	cps, workers := state.Controlplanes, state.Workers
	r.flatten(cl, &state)
	// Keep create-time sizing — mgmt mutates clusters.workers on add/remove node.
	state.Controlplanes = cps
	state.Workers = workers
	r.fetchKubeconfig(ctx, &state)
	r.ensureComputedKnown(&state)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *clusterResource) ModifyPlan(ctx context.Context, req resource.ModifyPlanRequest, resp *resource.ModifyPlanResponse) {
	if req.Plan.Raw.IsNull() {
		return // destroy
	}

	var plan clusterModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}

	// Create: explicit null from HCL (var default) must become Unknown so API
	// dual-stack defaults can be written without an inconsistent-result error.
	if req.State.Raw.IsNull() {
		if plan.PodSubnetIPv6.IsNull() {
			plan.PodSubnetIPv6 = types.StringUnknown()
		}
		if plan.ServiceSubnetIPv6.IsNull() {
			plan.ServiceSubnetIPv6 = types.StringUnknown()
		}
		resp.Diagnostics.Append(resp.Plan.Set(ctx, &plan)...)
		return
	}

	var state clusterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}

	// Ignore HCL drift on sizing after create (scale via pertisk_node).
	plan.Controlplanes = state.Controlplanes
	plan.Workers = state.Workers
	// Keep API-filled IPv6 CIDRs when config still omits them.
	if plan.PodSubnetIPv6.IsNull() {
		plan.PodSubnetIPv6 = state.PodSubnetIPv6
	}
	if plan.ServiceSubnetIPv6.IsNull() {
		plan.ServiceSubnetIPv6 = state.ServiceSubnetIPv6
	}

	resp.Diagnostics.Append(resp.Plan.Set(ctx, &plan)...)
}

func (r *clusterResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan clusterModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	var state clusterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	// Freeze create-time counts from the plan (ModifyPlan already pinned them to state).
	cps, workers := plan.Controlplanes, plan.Workers

	timeout := plan.TimeoutMinutes.ValueInt64()
	if timeout <= 0 {
		timeout = 45
	}
	waitCtx, cancel := context.WithTimeout(ctx, time.Duration(timeout)*time.Minute)
	defer cancel()

	if plan.K8sVersion.ValueString() != state.K8sVersion.ValueString() {
		tflog.Info(ctx, "upgrading cluster", map[string]any{
			"id":   state.ID.ValueString(),
			"from": state.K8sVersion.ValueString(),
			"to":   plan.K8sVersion.ValueString(),
		})
		up, err := r.client.UpgradeCluster(waitCtx, state.ID.ValueString(), plan.K8sVersion.ValueString())
		if err != nil {
			resp.Diagnostics.AddError("Upgrade cluster failed", err.Error())
			return
		}
		if up.JobID != "" {
			if _, err := r.client.WaitJob(waitCtx, up.JobID, 5*time.Second); err != nil {
				resp.Diagnostics.AddError("Upgrade cluster job failed", err.Error())
				return
			}
		}
	}

	cl, err := r.client.GetCluster(waitCtx, state.ID.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Read cluster after update failed", err.Error())
		return
	}
	r.flatten(cl, &plan)
	plan.Controlplanes = cps
	plan.Workers = workers
	plan.TimeoutMinutes = types.Int64Value(timeout)
	r.fetchKubeconfig(ctx, &plan)
	r.ensureComputedKnown(&plan)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *clusterResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state clusterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	timeout := state.TimeoutMinutes.ValueInt64()
	if timeout <= 0 {
		timeout = 45
	}
	waitCtx, cancel := context.WithTimeout(ctx, time.Duration(timeout)*time.Minute)
	defer cancel()

	del, err := r.client.DeleteCluster(waitCtx, state.ID.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			return
		}
		resp.Diagnostics.AddError("Delete cluster failed", err.Error())
		return
	}

	if del.Mode == "async" && del.JobID != "" {
		if _, err := r.client.WaitJob(waitCtx, del.JobID, 5*time.Second); err != nil {
			resp.Diagnostics.AddError("Delete cluster job failed", err.Error())
			return
		}
	}
	if err := r.client.WaitClusterGone(waitCtx, state.ID.ValueString(), 3*time.Second); err != nil {
		resp.Diagnostics.AddError("Waiting for cluster delete failed", err.Error())
		return
	}
}

func (r *clusterResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("id"), req, resp)
}

func (r *clusterResource) flatten(cl *client.Cluster, m *clusterModel) {
	m.ID = types.StringValue(cl.ID)
	m.Name = types.StringValue(cl.Name)
	m.ProviderID = types.StringValue(cl.ProviderID)
	m.Controlplanes = types.Int64Value(cl.Controlplanes)
	m.Workers = types.Int64Value(cl.Workers)
	m.NetworkMode = types.StringValue(cl.NetworkMode)
	m.CNI = types.StringValue(cl.CNI)
	m.K8sVersion = types.StringValue(cl.K8sVersion)
	m.CPMemory = types.Int64Value(cl.CPMemory)
	m.CPCores = types.Int64Value(cl.CPCores)
	m.CPDiskGB = types.Int64Value(cl.CPDiskGB)
	m.WorkerMemory = types.Int64Value(cl.WorkerMemory)
	m.WorkerCores = types.Int64Value(cl.WorkerCores)
	m.WorkerDiskGB = types.Int64Value(cl.WorkerDiskGB)
	m.MaxPods = types.Int64Value(cl.MaxPods)
	m.PodSubnet = types.StringValue(cl.PodSubnet)
	m.ServiceSubnet = types.StringValue(cl.ServiceSubnet)
	m.Status = types.StringValue(cl.Status)
	m.Arch = types.StringValue(cl.Arch)

	if cl.CPVMID != nil {
		m.CPVMID = types.Int64Value(*cl.CPVMID)
	} else if m.CPVMID.IsUnknown() {
		m.CPVMID = types.Int64Null()
	}
	m.VIP = optionalString(cl.VIP)
	m.VIP6 = optionalString(cl.VIP6)
	m.Endpoint = optionalString(cl.Endpoint)
	m.PodSubnetIPv6 = optionalString(cl.PodSubnetIPv6)
	m.ServiceSubnetIPv6 = optionalString(cl.ServiceSubnetIPv6)
	if m.TimeoutMinutes.IsNull() || m.TimeoutMinutes.IsUnknown() {
		m.TimeoutMinutes = types.Int64Value(45)
	}
}

func (r *clusterResource) ensureComputedKnown(m *clusterModel) {
	if m.Status.IsNull() || m.Status.IsUnknown() {
		m.Status = types.StringNull()
	}
	if m.Endpoint.IsNull() || m.Endpoint.IsUnknown() {
		m.Endpoint = types.StringNull()
	}
	if m.Kubeconfig.IsNull() || m.Kubeconfig.IsUnknown() {
		m.Kubeconfig = types.StringNull()
	}
	if m.Arch.IsNull() || m.Arch.IsUnknown() {
		m.Arch = types.StringNull()
	}
	if m.VIP.IsUnknown() {
		m.VIP = types.StringNull()
	}
	if m.VIP6.IsUnknown() {
		m.VIP6 = types.StringNull()
	}
	if m.PodSubnetIPv6.IsUnknown() {
		m.PodSubnetIPv6 = types.StringNull()
	}
	if m.ServiceSubnetIPv6.IsUnknown() {
		m.ServiceSubnetIPv6 = types.StringNull()
	}
}

func (r *clusterResource) fetchKubeconfig(ctx context.Context, m *clusterModel) {
	if r.client == nil || m.ID.IsNull() || m.ID.ValueString() == "" {
		m.Kubeconfig = types.StringNull()
		return
	}
	if m.Status.ValueString() != "ready" {
		m.Kubeconfig = types.StringNull()
		return
	}
	kc, err := r.client.GetKubeconfig(ctx, m.ID.ValueString())
	if err != nil {
		tflog.Warn(ctx, "kubeconfig not available yet", map[string]any{"error": err.Error()})
		m.Kubeconfig = types.StringNull()
		return
	}
	m.Kubeconfig = types.StringValue(kc)
}

func optionalString(v *string) types.String {
	if v == nil || *v == "" {
		return types.StringNull()
	}
	return types.StringValue(*v)
}
