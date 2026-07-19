# Railway Procfile for okf-mcp
# Deploys the outbox-worker daemon that syncs Supabase -> GitHub and reconciles GitHub -> Supabase.
# Required env vars in Railway:
#   POSTGRES_URL  (or use the auto-provided DATABASE_URL from Railway Postgres)
#   GITHUB_TOKEN
#   GITHUB_REPO   (format: owner/repo)
#   GEMINI_API_KEY (optional, for embeddings)
worker: ./target/release/outbox-worker
