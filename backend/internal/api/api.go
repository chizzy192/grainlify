package api

import (
	"log/slog"
	"strings"
	"time"

	"github.com/gofiber/fiber/v2"
	"github.com/gofiber/fiber/v2/middleware/cors"
	"github.com/gofiber/fiber/v2/middleware/logger"
	"github.com/gofiber/fiber/v2/middleware/recover"
	"github.com/gofiber/fiber/v2/middleware/requestid"

	"github.com/jagadeesh/grainlify/backend/internal/auth"
	"github.com/jagadeesh/grainlify/backend/internal/bus"
	"github.com/jagadeesh/grainlify/backend/internal/config"
	"github.com/jagadeesh/grainlify/backend/internal/db"
	"github.com/jagadeesh/grainlify/backend/internal/handlers"
)

type Deps struct {
	DB  *db.DB
	Bus bus.Bus
}

func New(cfg config.Config, deps Deps) *fiber.App {
	slog.Info("initializing Fiber app",
		"app_name", "grainlify-api",
	)
	app := fiber.New(fiber.Config{
		AppName:      "grainlify-api",
		IdleTimeout:  60 * time.Second,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 10 * time.Second,
	})
	slog.Info("Fiber app created")

	// Baseline middleware.
	app.Use(requestid.New())
	app.Use(recover.New())

	// Configure CORS from environment variables
	corsConfig := cors.Config{
		AllowHeaders:     "Origin, Content-Type, Accept, Authorization, X-Admin-Bootstrap-Token",
		AllowMethods:     "GET,POST,PUT,PATCH,DELETE,OPTIONS",
		AllowCredentials: true,
	}

	// If CORS_ORIGINS is set, use it (comma-separated list)
	if cfg.CORSOrigins != "" {
		origins := strings.Split(cfg.CORSOrigins, ",")
		for i := range origins {
			origins[i] = strings.TrimSpace(origins[i])
		}
		corsConfig.AllowOrigins = strings.Join(origins, ",")
	} else {
		// Otherwise, use AllowOriginsFunc for dynamic checking
		corsConfig.AllowOriginsFunc = func(origin string) bool {
			// Always allow localhost origins for development
			if strings.HasPrefix(origin, "http://localhost:") || strings.HasPrefix(origin, "http://127.0.0.1:") {
				return true
			}
			// If FrontendBaseURL is set, allow it
			if cfg.FrontendBaseURL != "" {
				if origin == cfg.FrontendBaseURL || strings.HasPrefix(origin, cfg.FrontendBaseURL+"/") {
					return true
				}
			}
			return false
		}
	}

	app.Use(cors.New(corsConfig))
	app.Use(logger.New())

	// Routes.
	app.Get("/", func(c *fiber.Ctx) error {
		return c.Status(fiber.StatusOK).JSON(fiber.Map{
			"service": "grainlify-api",
			"status":  "running",
			"version": "1.0.0",
		})
	})
	app.Get("/health", handlers.Health())
	app.Get("/ready", handlers.Ready(deps.DB))

	authHandler := handlers.NewAuthHandler(cfg, deps.DB)
	authGroup := app.Group("/auth")
	app.Get("/me", auth.RequireAuth(cfg.JWTSecret), authHandler.Me())

	// User profile endpoints
	userProfile := handlers.NewUserProfileHandler(cfg, deps.DB)
	app.Get("/profile", auth.RequireAuth(cfg.JWTSecret), userProfile.Profile())
	app.Get("/profile/calendar", auth.RequireAuth(cfg.JWTSecret), userProfile.ContributionCalendar())
	app.Get("/profile/activity", auth.RequireAuth(cfg.JWTSecret), userProfile.ContributionActivity())

	ghOAuth := handlers.NewGitHubOAuthHandler(cfg, deps.DB)
	// GitHub-only login/signup:
	authGroup.Get("/github/login/start", ghOAuth.LoginStart())
	// Alias to unified callback (for backwards compatibility with older callback URLs).
	authGroup.Get("/github/login/callback", ghOAuth.CallbackUnified())

	// Legacy "link GitHub to existing account" endpoints (still available).
	authGroup.Post("/github/start", auth.RequireAuth(cfg.JWTSecret), ghOAuth.Start())
	authGroup.Get("/github/callback", ghOAuth.CallbackUnified())
	authGroup.Get("/github/status", auth.RequireAuth(cfg.JWTSecret), ghOAuth.Status())

	// GitHub App installation endpoints
	ghApp := handlers.NewGitHubAppHandler(cfg, deps.DB)
	authGroup.Post("/github/app/install/start", auth.RequireAuth(cfg.JWTSecret), ghApp.StartInstallation())
	app.Get("/auth/github/app/install/callback", ghApp.HandleInstallationCallback())

	// KYC verification endpoints
	kyc := handlers.NewKYCHandler(cfg, deps.DB)
	authGroup.Post("/kyc/start", auth.RequireAuth(cfg.JWTSecret), kyc.Start())
	authGroup.Get("/kyc/status", auth.RequireAuth(cfg.JWTSecret), kyc.Status())

	// Public ecosystems list (includes computed project_count and user_count).
	ecosystems := handlers.NewEcosystemsPublicHandler(deps.DB)
	app.Get("/ecosystems", ecosystems.ListActive())

	// Public projects list with filtering
	projectsPublic := handlers.NewProjectsPublicHandler(deps.DB)
	app.Get("/projects", projectsPublic.List())
	app.Get("/projects/filters", projectsPublic.FilterOptions())

	projects := handlers.NewProjectsHandler(cfg, deps.DB)
	app.Post("/projects", auth.RequireAuth(cfg.JWTSecret), projects.Create())
	app.Get("/projects/mine", auth.RequireAuth(cfg.JWTSecret), projects.Mine())
	app.Post("/projects/:id/verify", auth.RequireAuth(cfg.JWTSecret), projects.Verify())

	sync := handlers.NewSyncHandler(deps.DB)
	app.Post("/projects/:id/sync", auth.RequireAuth(cfg.JWTSecret), sync.EnqueueFullSync())
	app.Get("/projects/:id/sync/jobs", auth.RequireAuth(cfg.JWTSecret), sync.JobsForProject())

	data := handlers.NewProjectDataHandler(deps.DB)
	app.Get("/projects/:id/issues", auth.RequireAuth(cfg.JWTSecret), data.Issues())
	app.Get("/projects/:id/prs", auth.RequireAuth(cfg.JWTSecret), data.PRs())
	app.Get("/projects/:id/events", auth.RequireAuth(cfg.JWTSecret), data.Events())

	admin := handlers.NewAdminHandler(cfg, deps.DB)
	adminGroup := app.Group("/admin", auth.RequireAuth(cfg.JWTSecret))
	adminGroup.Post("/bootstrap", admin.BootstrapAdmin())
	adminGroup.Get("/users", auth.RequireRole("admin"), admin.ListUsers())
	adminGroup.Put("/users/:id/role", auth.RequireRole("admin"), admin.SetUserRole())

	ecosystemsAdmin := handlers.NewEcosystemsAdminHandler(deps.DB)
	adminGroup.Get("/ecosystems", auth.RequireRole("admin"), ecosystemsAdmin.List())
	adminGroup.Post("/ecosystems", auth.RequireRole("admin"), ecosystemsAdmin.Create())
	adminGroup.Put("/ecosystems/:id", auth.RequireRole("admin"), ecosystemsAdmin.Update())

	webhooks := handlers.NewGitHubWebhooksHandler(cfg, deps.DB, deps.Bus)
	// Register webhook endpoint with explicit OPTIONS support for CORS
	app.Options("/webhooks/github", func(c *fiber.Ctx) error {
		return c.SendStatus(fiber.StatusOK)
	})
	app.Post("/webhooks/github", webhooks.Receive())

	// Didit webhook handler (supports both GET callback redirects and POST webhook events)
	diditWebhook := handlers.NewDiditWebhookHandler(cfg, deps.DB)
	app.Get("/webhooks/didit", diditWebhook.Receive())
	app.Post("/webhooks/didit", diditWebhook.Receive())

	slog.Info("all routes registered",
		"total_routes", "~30",
		"db_configured", deps.DB != nil,
		"nats_configured", deps.Bus != nil,
	)

	return app
}
