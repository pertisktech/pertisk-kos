package provider

import (
	"context"
	"os"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/provider/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/pertisk-tech/pertisk-kos/tools/terraform-provider-pertisk/internal/client"
)

var _ provider.Provider = &PertiskProvider{}

type PertiskProvider struct {
	version string
}

type providerModel struct {
	URL      types.String `tfsdk:"url"`
	Username types.String `tfsdk:"username"`
	Password types.String `tfsdk:"password"`
	Token    types.String `tfsdk:"token"`
	Insecure types.Bool   `tfsdk:"insecure"`
}

func New(version string) func() provider.Provider {
	return func() provider.Provider {
		return &PertiskProvider{version: version}
	}
}

func (p *PertiskProvider) Metadata(_ context.Context, _ provider.MetadataRequest, resp *provider.MetadataResponse) {
	resp.TypeName = "pertisk"
	resp.Version = p.version
}

func (p *PertiskProvider) Schema(_ context.Context, _ provider.SchemaRequest, resp *provider.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Manage Pertisk Kubernetes clusters via pertisk-mgmt.",
		Attributes: map[string]schema.Attribute{
			"url": schema.StringAttribute{
				MarkdownDescription: "Base URL of pertisk-mgmt (e.g. https://ptkos.example). Env: PERTISK_URL.",
				Optional:            true,
			},
			"username": schema.StringAttribute{
				MarkdownDescription: "Local auth username. Env: PERTISK_USERNAME. Ignored when token is set.",
				Optional:            true,
			},
			"password": schema.StringAttribute{
				MarkdownDescription: "Local auth password. Env: PERTISK_PASSWORD. Ignored when token is set.",
				Optional:            true,
				Sensitive:           true,
			},
			"token": schema.StringAttribute{
				MarkdownDescription: "Bearer JWT. Env: PERTISK_TOKEN. If set, login is skipped.",
				Optional:            true,
				Sensitive:           true,
			},
			"insecure": schema.BoolAttribute{
				MarkdownDescription: "Skip TLS verify (lab self-signed certs). Env: PERTISK_INSECURE=1.",
				Optional:            true,
			},
		},
	}
}

func (p *PertiskProvider) Configure(ctx context.Context, req provider.ConfigureRequest, resp *provider.ConfigureResponse) {
	var cfg providerModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &cfg)...)
	if resp.Diagnostics.HasError() {
		return
	}

	url := envOr("PERTISK_URL", cfg.URL.ValueString())
	username := envOr("PERTISK_USERNAME", cfg.Username.ValueString())
	password := envOr("PERTISK_PASSWORD", cfg.Password.ValueString())
	token := envOr("PERTISK_TOKEN", cfg.Token.ValueString())

	insecure := false
	if !cfg.Insecure.IsNull() && !cfg.Insecure.IsUnknown() {
		insecure = cfg.Insecure.ValueBool()
	} else if os.Getenv("PERTISK_INSECURE") == "1" || os.Getenv("PERTISK_INSECURE") == "true" {
		insecure = true
	}

	if url == "" {
		resp.Diagnostics.AddAttributeError(
			path.Root("url"),
			"Missing Pertisk mgmt URL",
			"Set provider url or PERTISK_URL.",
		)
		return
	}

	c := client.New(url, insecure)
	if token != "" {
		c.Token = token
	} else {
		if username == "" || password == "" {
			resp.Diagnostics.AddError(
				"Missing credentials",
				"Set token / PERTISK_TOKEN, or username+password / PERTISK_USERNAME+PERTISK_PASSWORD.",
			)
			return
		}
		if err := c.Login(ctx, username, password); err != nil {
			resp.Diagnostics.AddError("Login failed", err.Error())
			return
		}
	}

	resp.DataSourceData = c
	resp.ResourceData = c
}

func (p *PertiskProvider) Resources(_ context.Context) []func() resource.Resource {
	return []func() resource.Resource{
		NewProviderResource,
		NewClusterResource,
		NewNodeResource,
	}
}

func (p *PertiskProvider) DataSources(_ context.Context) []func() datasource.DataSource {
	return []func() datasource.DataSource{
		NewProviderDataSource,
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
