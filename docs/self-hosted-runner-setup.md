# Self-Hosted GitHub Actions Runner Setup — Mac Mini

This guide sets up a Mac mini as a self-hosted GitHub Actions runner for the
`pulseai-labs/PulseDB` repository, configured to run the Functional QA workflow
using Droid with your GLM5.2 custom model.

## Prerequisites

- Mac mini running macOS (Apple Silicon or Intel)
- Admin access to the `pulseai-labs/PulseDB` GitHub repo
- The Mac mini should be always-on (or wake-on-LAN) if you want QA to run on every PR

---

## ⚠️ Security: Dedicated User Account (READ THIS FIRST)

**Self-hosted runners have NO sandbox.** The runner executes as the macOS user that
registered it, with that user's full filesystem + network access. For a PUBLIC repo
(this repo is public), this is a real attack surface: anyone can open a PR, and the
workflow runs whatever code is in the workflow file.

**Two layers of protection are in place:**

### Layer 1: Workflow actor restriction

The QA workflow (`qa.yml`) has `if: github.actor == 'draco28'` — it **only runs when YOU
open or push to a PR.** Other contributors' PRs skip the self-hosted job entirely. This
means untrusted PR code never reaches your Mac mini.

### Layer 2: Dedicated macOS user account

On the Mac mini, **do NOT register the runner under your personal user account.** Create
a dedicated, restricted user:

```bash
# Create a standard (non-admin) user for the runner
sudo sysadminctl -addUser github-runner -password - -admin no

# Switch to the runner user to install everything
su - github-runner
```

The `github-runner` user:
- Is a **standard user** (no admin / sudo)
- Has its own home directory (`/Users/github-runner/`) — isolated from your personal files
- Has its own `~/.factory/` (separate droid login + GLM model config)
- **Cannot read** your personal home directory, SSH keys, browser data, or personal credentials
- Has Rust + droid installed under its own home

This way, even if a future workflow change accidentally runs untrusted code, the blast
radius is limited to the `github-runner` user's isolated home directory.

---

---

## Step 1: Register the Runner on GitHub

1. Go to: https://github.com/pulseai-labs/PulseDB/settings/actions/runners
2. Click **"New self-hosted runner"** → select **macOS** → select your architecture (ARM64 for Apple Silicon, x64 for Intel)
3. GitHub shows a registration token + a series of commands. **Copy the token** — you'll need it below.

## Step 2: Install the Runner (as the `github-runner` user)

Log in as `github-runner` on the Mac mini, open Terminal:

```bash
# Create a directory for the runner
mkdir ~/actions-runner && cd ~/actions-runner

# Download the runner (ARM64 — Apple Silicon. Use x64 for Intel)
curl -o actions-runner-osx-arm64-2.322.0.tar.gz -L \
  https://github.com/actions/runner/releases/download/v2.322.0/actions-runner-osx-arm64-2.322.0.tar.gz

# Extract
tar xzf actions-runner-osx-arm64-2.322.0.tar.gz

# Configure (replace TOKEN with the token from Step 1)
./config.sh --url https://github.com/pulseai-labs/PulseDB \
  --token <YOUR_TOKEN> \
  --name mac-mini-qa \
  --labels self-hosted,macos,arm64 \
  --work _work

# Install as a launchd service (starts on boot — needs admin)
# Run this from YOUR admin account, not the github-runner account:
sudo ./svc.sh install github-runner
sudo ./svc.sh start
```

Verify it's running:
```bash
sudo ./svc.sh status
```

The runner should show as "Idle" on https://github.com/pulseai-labs/PulseDB/settings/actions/runners

---

## Step 3: Install Rust Toolchain (as `github-runner`)

```bash
# Still logged in as github-runner
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustc --version  # should show 1.89+
```

---

## Step 4: Install + Authenticate Droid (as `github-runner`)

```bash
# Install droid
curl -fsSL https://app.factory.ai/cli | sh

# Authenticate (opens browser — log in with your Factory account)
droid login

# Verify
droid auth status
```

---

## Step 5: Configure the GLM5.2 Custom Model (as `github-runner`)

The custom model config lives in `~/.factory/settings.json` under the `github-runner`
user's home. This is a SEPARATE config from your personal Mac — the API key lives only
in the runner user's isolated home.

```bash
# Edit the settings file
nano ~/.factory/settings.json
```

Add the `customModels` + `sessionDefaultSettings` sections (same as your main Mac):

```json
{
  "customModels": [
    {
      "model": "glm-5.2",
      "id": "custom:GLM-[Z.AI-Coding-Plan]---Openai-0",
      "index": 0,
      "baseUrl": "https://api.z.ai/api/coding/paas/v4",
      "apiKey": "<YOUR_ZAI_API_KEY>",
      "displayName": "GLM [Z.AI Coding Plan] - Openai",
      "maxOutputTokens": 131072,
      "noImageSupport": true,
      "provider": "generic-chat-completion-api"
    }
  ],
  "sessionDefaultSettings": {
    "model": "custom:GLM-[Z.AI-Coding-Plan]---Openai-0",
    "reasoningEffort": "max",
    "autonomyMode": "spec",
    "interactionMode": "spec",
    "autonomyLevel": "high"
  }
}
```

Verify Droid can run headless with the custom model:

```bash
droid exec --auto high -m "custom:GLM-[Z.AI-Coding-Plan]---Openai-0" "echo hello world"
```

---

## Step 7: Cache the ONNX Model (optional, for builtin-embeddings tests)

```bash
# Trigger the model download (happens automatically on first QA run, but pre-caching saves time)
mkdir -p ~/Library/Caches/pulsedb/models/all-MiniLM-L6-v2
# The QA workflow will download it on first run if not cached
```

---

## Step 6: No manual skill installation needed

The QA skills (`qa/`, `qa-library/`) live in the AI workspace repo, not the canonical
repo. The workflow handles this automatically: it clones the AI workspace at runtime and
copies the skills into the checkout's `.factory/skills/` directory before running droid.
This means:

- **No global skill install** on the Mac mini — skills are copied per-PR-run
- **No cross-project collision** — each project's workflow copies its own skills from its
  own AI workspace repo
- **Always up-to-date** — the workflow clones fresh each time, so skill updates in the AI
  workspace are picked up automatically

The AI workspace repo (`pulseai-labs/pulsedb-internal`) must be **cloneable by the runner's
GitHub credentials.** If it's private, the runner needs either:
- SSH key access (add the `github-runner` user's public SSH key as a deploy key)
- Or a personal access token with repo read access (stored as a git credential helper)

---

## Step 7: Cache the ONNX Model (optional, for builtin-embeddings tests)

```bash
# Trigger the model download (happens automatically on first QA run, but pre-caching saves time)
mkdir -p ~/Library/Caches/pulsedb/models/all-MiniLM-L6-v2
# The QA workflow will download it on first run if not cached
```

---

## Step 8: Verify the Runner Picks Up Jobs

1. Push any commit to a PR branch, or trigger the workflow manually
2. Check https://github.com/pulseai-labs/PulseDB/actions — the QA job should show
   "Runner: mac-mini-qa" and run on your Mac mini

---

## Troubleshooting

**Runner not picking up jobs:**
- Check `sudo ./svc.sh status` on the Mac mini
- Check the runner is "Idle" (not "Offline") on the GitHub settings page
- Ensure the runner labels match `self-hosted` (the workflow uses `runs-on: self-hosted`)

**Droid auth fails in CI:**
- Run `droid login` again on the Mac mini
- Check `~/.factory/auth.json` exists and has `access_token` + `refresh_token`
- OAuth tokens expire; re-login if the runner hasn't been used in a while

**Rust compile fails:**
- Check `rustc --version` ≥ 1.89 (the MSRV)
- Run `rustup update` to get the latest stable

**GLM model not found:**
- Verify `~/.factory/settings.json` has the `customModels` entry
- Verify the model ID matches: `custom:GLM-[Z.AI-Coding-Plan]---Openai-0`
- Test with `droid exec -m "custom:GLM-[Z.AI-Coding-Plan]---Openai-0" "hello"`

**QA skill not found:**
- The workflow auto-clones the AI workspace (`pulseai-labs/pulsedb-internal`) and copies
  the skills into `.factory/skills/` in the checkout — no manual install needed
- If the clone fails (private repo), the runner needs SSH or token access to the AI workspace
- Check the "Install QA skills into checkout" step logs for clone errors
- The workflow prompt references project-level `.factory/skills/qa/SKILL.md` (not global)

---

## What the Workflow Does on the Runner

When a PR is opened or pushed:

1. Checks out the PR branch code
2. Runs `droid exec --auto high -m "custom:GLM-[Z.AI-Coding-Plan]---Openai-0"` with the QA skill prompt
3. Droid reads the git diff, writes consumer-simulation Rust test programs, compiles + runs them
4. Posts the QA report as a PR comment
5. Uploads artifacts (report + output)

The runner uses the Mac mini's local Droid auth + GLM model config — **no GitHub secrets needed**.
