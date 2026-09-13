package client

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Client talks to pertisk-mgmt over HTTP + Bearer JWT.
type Client struct {
	BaseURL    string
	Token      string
	HTTPClient *http.Client
}

type LoginResponse struct {
	Token    string `json:"token"`
	Username string `json:"username"`
	Role     string `json:"role"`
}

type CreateClusterRequest struct {
	Name              string  `json:"name"`
	ProviderID        string  `json:"provider_id"`
	Controlplanes     int64   `json:"controlplanes"`
	Workers           int64   `json:"workers"`
	NetworkMode       string  `json:"network_mode,omitempty"`
	VIP               *string `json:"vip,omitempty"`
	VIP6              *string `json:"vip6,omitempty"`
	CNI               string  `json:"cni,omitempty"`
	K8sVersion        string  `json:"k8s_version,omitempty"`
	CPMemory          int64   `json:"cp_memory,omitempty"`
	CPCores           int64   `json:"cp_cores,omitempty"`
	CPDiskGB          int64   `json:"cp_disk_gb,omitempty"`
	WorkerMemory      int64   `json:"worker_memory,omitempty"`
	WorkerCores       int64   `json:"worker_cores,omitempty"`
	WorkerDiskGB      int64   `json:"worker_disk_gb,omitempty"`
	CPVMID            int64   `json:"cp_vmid,omitempty"`
	MaxPods           int64   `json:"max_pods,omitempty"`
	Arch              *string `json:"arch,omitempty"`
	PodSubnet         string  `json:"pod_subnet,omitempty"`
	ServiceSubnet     string  `json:"service_subnet,omitempty"`
	PodSubnetIPv6     *string `json:"pod_subnet_ipv6,omitempty"`
	ServiceSubnetIPv6 *string `json:"service_subnet_ipv6,omitempty"`
	ReuseAddons       bool    `json:"reuse_addons"`
	AddonPreset       *string `json:"addon_preset,omitempty"`
}

type CreateClusterResponse struct {
	ID     string `json:"id"`
	JobID  string `json:"job_id"`
	Status string `json:"status"`
}

type DeleteClusterResponse struct {
	OK         bool   `json:"ok"`
	Mode       string `json:"mode"`
	JobID      string `json:"job_id"`
	ProviderID string `json:"provider_id"`
}

type JobIDResponse struct {
	JobID string `json:"job_id"`
}

type Node struct {
	ID           string  `json:"id"`
	ClusterID    string  `json:"cluster_id"`
	Name         string  `json:"name"`
	Role         string  `json:"role"`
	VMID         *int64  `json:"vmid"`
	IP           *string `json:"ip"`
	IP6          *string `json:"ip6"`
	K8sVersion   *string `json:"k8s_version"`
	Memory       *int64  `json:"memory"`
	Cores        *int64  `json:"cores"`
	DiskGB       *int64  `json:"disk_gb"`
	Source       string  `json:"source"`
	Status       string  `json:"status"`
	Availability string  `json:"availability"`
	CreatedAt    string  `json:"created_at"`
	UpdatedAt    string  `json:"updated_at"`
}

type AddNodeRequest struct {
	Role   string `json:"role"`
	Count  int64  `json:"count,omitempty"`
	Memory *int64 `json:"memory,omitempty"`
	Cores  *int64 `json:"cores,omitempty"`
	DiskGB *int64 `json:"disk_gb,omitempty"`
}

type AdoptNodeRequest struct {
	Role   string  `json:"role"`
	IP     string  `json:"ip"`
	Name   *string `json:"name,omitempty"`
	Source string  `json:"source,omitempty"`
}

type Job struct {
	ID         string  `json:"id"`
	ClusterID  *string `json:"cluster_id"`
	Kind       string  `json:"kind"`
	Status     string  `json:"status"`
	Error      *string `json:"error"`
	CreatedAt  string  `json:"created_at"`
	UpdatedAt  string  `json:"updated_at"`
	FinishedAt *string `json:"finished_at"`
}

type Cluster struct {
	ID                string  `json:"id"`
	Name              string  `json:"name"`
	ProviderID        string  `json:"provider_id"`
	ProviderName      *string `json:"provider_name"`
	ProviderKind      *string `json:"provider_kind"`
	Status            string  `json:"status"`
	Availability      string  `json:"availability"`
	Controlplanes     int64   `json:"controlplanes"`
	Workers           int64   `json:"workers"`
	VIP               *string `json:"vip"`
	VIP6              *string `json:"vip6"`
	CNI               string  `json:"cni"`
	K8sVersion        string  `json:"k8s_version"`
	CPMemory          int64   `json:"cp_memory"`
	CPCores           int64   `json:"cp_cores"`
	CPDiskGB          int64   `json:"cp_disk_gb"`
	WorkerMemory      int64   `json:"worker_memory"`
	WorkerCores       int64   `json:"worker_cores"`
	WorkerDiskGB      int64   `json:"worker_disk_gb"`
	CPVMID            *int64  `json:"cp_vmid"`
	Endpoint          *string `json:"endpoint"`
	Error             *string `json:"error"`
	NetworkMode       string  `json:"network_mode"`
	MaxPods           int64   `json:"max_pods"`
	Arch              string  `json:"arch"`
	PodSubnet         string  `json:"pod_subnet"`
	ServiceSubnet     string  `json:"service_subnet"`
	PodSubnetIPv6     *string `json:"pod_subnet_ipv6"`
	ServiceSubnetIPv6 *string `json:"service_subnet_ipv6"`
	CreatedAt         string  `json:"created_at"`
	UpdatedAt         string  `json:"updated_at"`
}

type ClusterDetail struct {
	Cluster Cluster `json:"cluster"`
}

type Provider struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Kind      string `json:"kind"`
	URL       string `json:"url"`
	TokenID   string `json:"token_id"`
	Node      string `json:"node"`
	Storage   string `json:"storage"`
	Bridge    string `json:"bridge"`
	Insecure  int64  `json:"insecure"`
	Arch      string `json:"arch"`
	CreatedAt string `json:"created_at"`
	UpdatedAt string `json:"updated_at"`
}

// ProviderWriteRequest is POST /api/providers.
type ProviderWriteRequest struct {
	Name        string `json:"name"`
	URL         string `json:"url"`
	TokenID     string `json:"token_id"`
	TokenSecret string `json:"token_secret"`
	Node        string `json:"node"`
	Storage     string `json:"storage"`
	Bridge      string `json:"bridge,omitempty"`
	Insecure    bool   `json:"insecure"`
	Kind        string `json:"kind,omitempty"`
	Arch        string `json:"arch,omitempty"`
}

// ProviderPatchRequest is PUT /api/providers/{id}.
type ProviderPatchRequest struct {
	Name        *string `json:"name,omitempty"`
	URL         *string `json:"url,omitempty"`
	TokenID     *string `json:"token_id,omitempty"`
	TokenSecret *string `json:"token_secret,omitempty"`
	Node        *string `json:"node,omitempty"`
	Storage     *string `json:"storage,omitempty"`
	Bridge      *string `json:"bridge,omitempty"`
	Insecure    *bool   `json:"insecure,omitempty"`
	Arch        *string `json:"arch,omitempty"`
}

type APIError struct {
	StatusCode int
	Body       string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("pertisk api %d: %s", e.StatusCode, e.Body)
}

func New(baseURL string, insecure bool) *Client {
	tr := http.DefaultTransport.(*http.Transport).Clone()
	if insecure {
		// #nosec G402 — lab self-signed certs (same as mgmt UI Insecure TLS)
		tr.TLSClientConfig = insecureTLSConfig()
	}
	return &Client{
		BaseURL: strings.TrimRight(baseURL, "/"),
		HTTPClient: &http.Client{
			Timeout:   60 * time.Second,
			Transport: tr,
		},
	}
}

func (c *Client) Login(ctx context.Context, username, password string) error {
	var out LoginResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/auth/login", map[string]string{
		"username": username,
		"password": password,
	}, &out); err != nil {
		return err
	}
	if out.Token == "" {
		return fmt.Errorf("login returned empty token")
	}
	c.Token = out.Token
	return nil
}

func (c *Client) ListProviders(ctx context.Context) ([]Provider, error) {
	var out []Provider
	if err := c.doJSON(ctx, http.MethodGet, "/api/providers", nil, &out); err != nil {
		return nil, err
	}
	return out, nil
}

func (c *Client) GetProvider(ctx context.Context, id string) (*Provider, error) {
	var out Provider
	if err := c.doJSON(ctx, http.MethodGet, "/api/providers/"+id, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) CreateProvider(ctx context.Context, req ProviderWriteRequest) (*Provider, error) {
	var out Provider
	if err := c.doJSON(ctx, http.MethodPost, "/api/providers", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) UpdateProvider(ctx context.Context, id string, req ProviderPatchRequest) (*Provider, error) {
	var out Provider
	if err := c.doJSON(ctx, http.MethodPut, "/api/providers/"+id, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteProvider(ctx context.Context, id string) error {
	return c.doJSON(ctx, http.MethodDelete, "/api/providers/"+id, nil, nil)
}

func (c *Client) GetKubeconfig(ctx context.Context, clusterID string) (string, error) {
	raw, err := c.doBytes(ctx, http.MethodGet, "/api/clusters/"+clusterID+"/kubeconfig", nil)
	if err != nil {
		return "", err
	}
	return string(raw), nil
}

func (c *Client) CreateCluster(ctx context.Context, req CreateClusterRequest) (*CreateClusterResponse, error) {
	var out CreateClusterResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/clusters", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) GetCluster(ctx context.Context, id string) (*Cluster, error) {
	var out ClusterDetail
	if err := c.doJSON(ctx, http.MethodGet, "/api/clusters/"+id, nil, &out); err != nil {
		return nil, err
	}
	return &out.Cluster, nil
}

func (c *Client) DeleteCluster(ctx context.Context, id string) (*DeleteClusterResponse, error) {
	var out DeleteClusterResponse
	if err := c.doJSON(ctx, http.MethodDelete, "/api/clusters/"+id, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) UpgradeCluster(ctx context.Context, id, version string) (*JobIDResponse, error) {
	var out JobIDResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/clusters/"+id+"/upgrade", map[string]string{
		"version": version,
	}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListNodes(ctx context.Context, clusterID string) ([]Node, error) {
	var out []Node
	if err := c.doJSON(ctx, http.MethodGet, "/api/clusters/"+clusterID+"/nodes", nil, &out); err != nil {
		return nil, err
	}
	return out, nil
}

func (c *Client) GetNode(ctx context.Context, clusterID, nodeID string) (*Node, error) {
	var out Node
	if err := c.doJSON(ctx, http.MethodGet, "/api/clusters/"+clusterID+"/nodes/"+nodeID, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) AddNode(ctx context.Context, clusterID string, req AddNodeRequest) (*JobIDResponse, error) {
	if req.Count == 0 {
		req.Count = 1
	}
	var out JobIDResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/clusters/"+clusterID+"/nodes", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) AdoptNode(ctx context.Context, clusterID string, req AdoptNodeRequest) (*JobIDResponse, error) {
	var out JobIDResponse
	if err := c.doJSON(ctx, http.MethodPost, "/api/clusters/"+clusterID+"/nodes/adopt", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) RemoveNode(ctx context.Context, clusterID, nodeID string) (*JobIDResponse, error) {
	var out JobIDResponse
	if err := c.doJSON(ctx, http.MethodDelete, "/api/clusters/"+clusterID+"/nodes/"+nodeID, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

type AddonSummary struct {
	ID     string  `json:"id"`
	Name   string  `json:"name"`
	Status string  `json:"status"`
	OK     bool    `json:"ok"`
	Error  *string `json:"error"`
	JobID  *string `json:"job_id"`
}

type InstallAddonResponse struct {
	OK    bool   `json:"ok"`
	JobID string `json:"job_id"`
	Addon string `json:"addon"`
}

func (c *Client) GetAddon(ctx context.Context, clusterID, addon string) (*AddonSummary, error) {
	var out AddonSummary
	path := "/api/clusters/" + clusterID + "/addons/" + addon
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) InstallAddon(ctx context.Context, clusterID, addon string, body map[string]any) (*InstallAddonResponse, error) {
	var out InstallAddonResponse
	path := "/api/clusters/" + clusterID + "/addons/" + addon + "/install"
	if err := c.doJSON(ctx, http.MethodPost, path, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) GetJob(ctx context.Context, id string) (*Job, error) {
	var out Job
	if err := c.doJSON(ctx, http.MethodGet, "/api/jobs/"+id, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// WaitJob polls until the job reaches a terminal status.
func (c *Client) WaitJob(ctx context.Context, jobID string, poll time.Duration) (*Job, error) {
	if poll <= 0 {
		poll = 5 * time.Second
	}
	ticker := time.NewTicker(poll)
	defer ticker.Stop()

	for {
		job, err := c.GetJob(ctx, jobID)
		if err != nil {
			return nil, err
		}
		switch job.Status {
		case "succeeded":
			return job, nil
		case "failed", "cancelled":
			msg := job.Status
			if job.Error != nil && *job.Error != "" {
				msg = *job.Error
			}
			return job, fmt.Errorf("job %s %s: %s", jobID, job.Status, msg)
		}

		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-ticker.C:
		}
	}
}

// WaitClusterGone polls until GET cluster returns 404 (delete finished).
func (c *Client) WaitClusterGone(ctx context.Context, id string, poll time.Duration) error {
	if poll <= 0 {
		poll = 5 * time.Second
	}
	ticker := time.NewTicker(poll)
	defer ticker.Stop()

	for {
		_, err := c.GetCluster(ctx, id)
		if err != nil {
			if apiErr, ok := err.(*APIError); ok && apiErr.StatusCode == http.StatusNotFound {
				return nil
			}
			return err
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
		}
	}
}

func (c *Client) doBytes(ctx context.Context, method, path string, body any) ([]byte, error) {
	var rdr io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		rdr = bytes.NewReader(b)
	}

	req, err := http.NewRequestWithContext(ctx, method, c.BaseURL+path, rdr)
	if err != nil {
		return nil, err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.Token != "" {
		req.Header.Set("Authorization", "Bearer "+c.Token)
	}

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, &APIError{StatusCode: resp.StatusCode, Body: strings.TrimSpace(string(raw))}
	}
	return raw, nil
}

func (c *Client) doJSON(ctx context.Context, method, path string, body any, out any) error {
	raw, err := c.doBytes(ctx, method, path, body)
	if err != nil {
		return err
	}
	if out == nil || len(raw) == 0 {
		return nil
	}
	if err := json.Unmarshal(raw, out); err != nil {
		return fmt.Errorf("decode %s %s: %w (body=%s)", method, path, err, truncate(string(raw), 200))
	}
	return nil
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
