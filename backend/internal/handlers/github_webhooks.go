package handlers

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/json"
	"log/slog"
	"strings"

	"github.com/gofiber/fiber/v2"

	"github.com/jagadeesh/grainlify/backend/internal/bus"
	"github.com/jagadeesh/grainlify/backend/internal/config"
	"github.com/jagadeesh/grainlify/backend/internal/db"
	"github.com/jagadeesh/grainlify/backend/internal/events"
	"github.com/jagadeesh/grainlify/backend/internal/ingest"
)

type GitHubWebhooksHandler struct {
	cfg config.Config
	db  *db.DB
	bus bus.Bus
	ing *ingest.GitHubWebhookIngestor
}

func NewGitHubWebhooksHandler(cfg config.Config, d *db.DB, b bus.Bus) *GitHubWebhooksHandler {
	var ingestor *ingest.GitHubWebhookIngestor
	if d != nil && d.Pool != nil {
		ingestor = &ingest.GitHubWebhookIngestor{Pool: d.Pool}
	}
	return &GitHubWebhooksHandler{cfg: cfg, db: d, bus: b, ing: ingestor}
}

func (h *GitHubWebhooksHandler) Receive() fiber.Handler {
	return func(c *fiber.Ctx) error {
		// Handle CORS preflight requests
		if c.Method() == "OPTIONS" {
			return c.SendStatus(fiber.StatusOK)
		}

		// Log all webhook requests immediately (before any checks)
		slog.Info("GitHub webhook endpoint hit",
			"method", c.Method(),
			"path", c.Path(),
			"original_url", c.OriginalURL(),
			"remote_ip", c.IP(),
			"user_agent", c.Get("User-Agent"),
		)

		if h.cfg.GitHubWebhookSecret == "" {
			slog.Error("webhook secret not configured")
			return c.Status(fiber.StatusServiceUnavailable).JSON(fiber.Map{"error": "webhook_secret_not_configured"})
		}

		delivery := strings.TrimSpace(c.Get("X-GitHub-Delivery"))
		event := strings.TrimSpace(c.Get("X-GitHub-Event"))
		sig := strings.TrimSpace(c.Get("X-Hub-Signature-256"))

		// Log webhook reception for debugging
		slog.Info("GitHub webhook received",
			"event", event,
			"delivery_id", delivery,
			"path", c.Path(),
			"method", c.Method(),
		)

		body := c.Body()

		if !verifyGitHubSignature(h.cfg.GitHubWebhookSecret, body, sig) {
			slog.Warn("GitHub webhook signature verification failed",
				"delivery_id", delivery,
				"event", event,
				"has_signature", sig != "",
			)
			return c.Status(fiber.StatusUnauthorized).JSON(fiber.Map{"error": "invalid_signature"})
		}

		var repoFullName string
		var action string

		var env ghWebhookEnvelope
		if err := json.Unmarshal(body, &env); err == nil {
			if env.Repository != nil {
				repoFullName = strings.TrimSpace(env.Repository.FullName)
			}
			action = strings.TrimSpace(env.Action)
		}

		ev := events.GitHubWebhookReceived{
			DeliveryID:   delivery,
			Event:        event,
			Action:       action,
			RepoFullName: repoFullName,
			Payload:      body,
		}

		// Preferred path: publish to NATS and return immediately (no heavy work in request path).
		if h.bus != nil {
			b, _ := json.Marshal(ev)
			_ = h.bus.Publish(c.Context(), events.SubjectGitHubWebhookReceived, b)
			return c.SendStatus(fiber.StatusOK)
		}

		// Fallback path (no NATS): ingest inline (still no external calls).
		if h.ing != nil {
			_ = h.ing.Ingest(c.Context(), ev)
		}

		// ACK fast.
		return c.SendStatus(fiber.StatusOK)
	}
}

func verifyGitHubSignature(secret string, body []byte, header string) bool {
	// GitHub uses: X-Hub-Signature-256: sha256=<hex>
	if !strings.HasPrefix(header, "sha256=") {
		return false
	}
	gotHex := strings.ToLower(strings.TrimPrefix(header, "sha256="))
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = mac.Write(body)
	want := mac.Sum(nil)
	wantHex := hexEncodeLower(want)
	return subtle.ConstantTimeCompare([]byte(gotHex), []byte(wantHex)) == 1
}

func hexEncodeLower(b []byte) string {
	const hextable = "0123456789abcdef"
	out := make([]byte, len(b)*2)
	for i, v := range b {
		out[i*2] = hextable[v>>4]
		out[i*2+1] = hextable[v&0x0f]
	}
	return string(out)
}

type ghWebhookEnvelope struct {
	Action     string         `json:"action"`
	Repository *ghRepoPayload `json:"repository"`
}

type ghRepoPayload struct {
	FullName string `json:"full_name"`
}

 


