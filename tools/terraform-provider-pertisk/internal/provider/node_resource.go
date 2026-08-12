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
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/int64planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringdefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/hashicorp/terraform-plugin-log/tflog"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var (
	_ resource.Resource                = &nodeResource{}
	_ resource.ResourceWithImportState = &nodeResource{}
)

type nodeResource struct {
	client *client.Client
}

type nodeModel struct {
	ID             types.String `tfsdk:"id"`
	ClusterID      types.String `tfsdk:"cluster_id"`
	Role           types.String `tfsdk:"role"`
	Mode           types.String `tfsdk:"mode"`
	IP             types.String `tfsdk:"ip"`
	Name           types.String `tfsdk:"name"`
	Source         types.String `tfsdk:"source"`
	Memory         types.Int64  `tfsdk:"memory"`
	Cores          types.Int64  `tfsdk:"cores"`
	DiskGB         types.Int64  `tfsdk:"disk_gb"`
	VMID           types.Int64  `tfsdk:"vmid"`
	Status         types.String `tfsdk:"status"`
	TimeoutMinutes types.Int64  `tfsdk:"timeout_minutes"`
}

func NewNodeResource() resource.Resource {
	return &nodeResource{}
}

func (r *nodeResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_node"
}

func (r *nodeResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Add a VM node or adopt an existing host into a Pertisk cluster. Proxmox only for mode=create (vSphere add-node is not supported by mgmt yet).",
		Attributes: map[string]schema.Attribute{
			"id": schema.StringAttribute{
				Computed: true,
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"cluster_id": schema.StringAttribute{
				Required: true,
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"role": schema.StringAttribute{
				Required:            true,
				MarkdownDescription: "controlplane | worker",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"mode": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				Default:             stringdefault.StaticString("create"),
				MarkdownDescription: "create (provision VM) | adopt (join existing IP).",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"ip": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "Required for mode=adopt (Machine API IPv4). Computed after create/join.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
				},
			},
			"name": schema.StringAttribute{
				Optional:            true,
				Computed:            true,
				MarkdownDescription: "Optional hostname for adopt; otherwise mgmt assigns {cluster}-cp-N / {cluster}-wk-N.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"source": schema.StringAttribute{
				Optional: true,
				Computed: true,
				// No StaticString default: mode=create gets proxmox|vsphere from the API.
				// Defaulting to "adopted" caused inconsistent apply results for create VMs.
				MarkdownDescription: "Provenance from API after apply (proxmox|vsphere|adopted|baremetal). Optional input for mode=adopt (adopted|baremetal); defaults to adopted.",
				PlanModifiers: []planmodifier.String{
					stringplanmodifier.RequiresReplace(),
					stringplanmodifier.UseStateForUnknown(),
				},
			},
			"memory": schema.Int64Attribute{
				Optional:            true,
				MarkdownDescription: "Optional memory MB override (mode=create).",
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"cores": schema.Int64Attribute{
				Optional: true,
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"disk_gb": schema.Int64Attribute{
				Optional: true,
				PlanModifiers: []planmodifier.Int64{
					int64planmodifier.RequiresReplace(),
				},
			},
			"vmid": schema.Int64Attribute{
				Computed: true,
			},
			"status": schema.StringAttribute{
				Computed: true,
			},
			"timeout_minutes": schema.Int64Attribute{
				Optional: true,
				Computed: true,
				Default:  int64default.StaticInt64(45),
			},
		},
	}
}

func (r *nodeResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
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

func (r *nodeResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan nodeModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	role := plan.Role.ValueString()
	if role != "controlplane" && role != "worker" {
		resp.Diagnostics.AddError("Invalid role", "role must be controlplane or worker")
		return
	}
	mode := plan.Mode.ValueString()
	clusterID := plan.ClusterID.ValueString()

	before, err := r.client.ListNodes(ctx, clusterID)
	if err != nil {
		resp.Diagnostics.AddError("List nodes failed", err.Error())
		return
	}
	beforeIDs := map[string]struct{}{}
	for _, n := range before {
		beforeIDs[n.ID] = struct{}{}
	}

	timeout := plan.TimeoutMinutes.ValueInt64()
	if timeout <= 0 {
		timeout = 45
	}
	waitCtx, cancel := context.WithTimeout(ctx, time.Duration(timeout)*time.Minute)
	defer cancel()

	var jobID string
	switch mode {
	case "create", "":
		addReq := client.AddNodeRequest{Role: role, Count: 1}
		if !plan.Memory.IsNull() && !plan.Memory.IsUnknown() {
			v := plan.Memory.ValueInt64()
			addReq.Memory = &v
		}
		if !plan.Cores.IsNull() && !plan.Cores.IsUnknown() {
			v := plan.Cores.ValueInt64()
			addReq.Cores = &v
		}
		if !plan.DiskGB.IsNull() && !plan.DiskGB.IsUnknown() {
			v := plan.DiskGB.ValueInt64()
			addReq.DiskGB = &v
		}
		out, err := r.client.AddNode(waitCtx, clusterID, addReq)
		if err != nil {
			resp.Diagnostics.AddError("Add node failed", err.Error())
			return
		}
		jobID = out.JobID
	case "adopt":
		ip := plan.IP.ValueString()
		if ip == "" {
			resp.Diagnostics.AddError("Missing ip", "mode=adopt requires ip")
			return
		}
		src := plan.Source.ValueString()
		if src == "" || plan.Source.IsNull() || plan.Source.IsUnknown() {
			src = "adopted"
		}
		adoptReq := client.AdoptNodeRequest{
			Role:   role,
			IP:     ip,
			Source: src,
		}
		if !plan.Name.IsNull() && !plan.Name.IsUnknown() && plan.Name.ValueString() != "" {
			n := plan.Name.ValueString()
			adoptReq.Name = &n
		}
		out, err := r.client.AdoptNode(waitCtx, clusterID, adoptReq)
		if err != nil {
			resp.Diagnostics.AddError("Adopt node failed", err.Error())
			return
		}
		jobID = out.JobID
	default:
		resp.Diagnostics.AddError("Invalid mode", "mode must be create or adopt")
		return
	}

	tflog.Info(ctx, "node job enqueued", map[string]any{"job_id": jobID, "mode": mode})
	if jobID != "" {
		if _, err := r.client.WaitJob(waitCtx, jobID, 5*time.Second); err != nil {
			resp.Diagnostics.AddError("Node job failed", err.Error())
			return
		}
	}

	node, err := r.findNewNode(waitCtx, clusterID, beforeIDs, mode, plan.IP.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Could not resolve new node", err.Error())
		return
	}

	r.flatten(node, &plan)
	plan.Mode = types.StringValue(mode)
	if plan.TimeoutMinutes.IsNull() || plan.TimeoutMinutes.IsUnknown() {
		plan.TimeoutMinutes = types.Int64Value(timeout)
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *nodeResource) findNewNode(ctx context.Context, clusterID string, before map[string]struct{}, mode, adoptIP string) (*client.Node, error) {
	deadline := time.Now().Add(2 * time.Minute)
	for {
		nodes, err := r.client.ListNodes(ctx, clusterID)
		if err != nil {
			return nil, err
		}
		var candidates []client.Node
		for _, n := range nodes {
			if _, ok := before[n.ID]; ok {
				continue
			}
			candidates = append(candidates, n)
		}
		if mode == "adopt" && adoptIP != "" {
			for i := range candidates {
				if candidates[i].IP != nil && *candidates[i].IP == adoptIP {
					return &candidates[i], nil
				}
			}
			// Also match existing nodes by IP if insert raced before snapshot.
			for i := range nodes {
				if nodes[i].IP != nil && *nodes[i].IP == adoptIP {
					return &nodes[i], nil
				}
			}
		} else if len(candidates) == 1 {
			return &candidates[0], nil
		} else if len(candidates) > 1 {
			// Prefer newest by created_at string (RFC3339 sorts lexicographically).
			best := &candidates[0]
			for i := 1; i < len(candidates); i++ {
				if candidates[i].CreatedAt > best.CreatedAt {
					best = &candidates[i]
				}
			}
			return best, nil
		}
		if time.Now().After(deadline) {
			return nil, fmt.Errorf("timed out waiting for new node to appear in inventory")
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(3 * time.Second):
		}
	}
}

func (r *nodeResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state nodeModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if r.client == nil {
		resp.Diagnostics.AddError("Client not configured", "Provider client is nil")
		return
	}

	node, err := r.client.GetNode(ctx, state.ClusterID.ValueString(), state.ID.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			resp.State.RemoveResource(ctx)
			return
		}
		resp.Diagnostics.AddError("Read node failed", err.Error())
		return
	}
	mode := state.Mode
	timeout := state.TimeoutMinutes
	mem, cores, disk := state.Memory, state.Cores, state.DiskGB
	r.flatten(node, &state)
	state.Mode = mode
	state.TimeoutMinutes = timeout
	// Preserve optional create-only hardware overrides from config/state.
	state.Memory = mem
	state.Cores = cores
	state.DiskGB = disk
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *nodeResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan nodeModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	var state nodeModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}

	// Hardware overrides are create-only; changing them requires replace.
	if !plan.Memory.Equal(state.Memory) || !plan.Cores.Equal(state.Cores) || !plan.DiskGB.Equal(state.DiskGB) {
		resp.Diagnostics.AddError(
			"Hardware overrides are create-only",
			"Changing memory/cores/disk_gb requires replacing the node (taint / recreate).",
		)
		return
	}

	state.TimeoutMinutes = plan.TimeoutMinutes
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *nodeResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state nodeModel
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

	out, err := r.client.RemoveNode(waitCtx, state.ClusterID.ValueString(), state.ID.ValueString())
	if err != nil {
		if apiErr, ok := err.(*client.APIError); ok && apiErr.StatusCode == http.StatusNotFound {
			return
		}
		resp.Diagnostics.AddError("Remove node failed", err.Error())
		return
	}
	if out.JobID != "" {
		if _, err := r.client.WaitJob(waitCtx, out.JobID, 5*time.Second); err != nil {
			resp.Diagnostics.AddError("Remove node job failed", err.Error())
			return
		}
	}
}

func (r *nodeResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	// Format: cluster_id/node_id
	parts := strings.SplitN(req.ID, "/", 2)
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		resp.Diagnostics.AddError(
			"Invalid import ID",
			"Expected format: <cluster_id>/<node_id>",
		)
		return
	}
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("cluster_id"), parts[0])...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("id"), parts[1])...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("mode"), "create")...)
}

func (r *nodeResource) flatten(n *client.Node, m *nodeModel) {
	m.ID = types.StringValue(n.ID)
	m.ClusterID = types.StringValue(n.ClusterID)
	m.Role = types.StringValue(n.Role)
	m.Name = types.StringValue(n.Name)
	m.Status = types.StringValue(n.Status)
	m.Source = types.StringValue(n.Source)
	m.IP = optionalString(n.IP)
	if n.VMID != nil {
		m.VMID = types.Int64Value(*n.VMID)
	} else {
		m.VMID = types.Int64Null()
	}
}
