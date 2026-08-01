# Self-Hosted GitHub Actions Runner Setup — Mac Mini

This guide sets up a Mac mini as a self-hosted GitHub Actions runner for the
`pulseai-labs/PulseDB` repository, configured to run the Functional QA workflow
using Droid with your GLM5.2 custom model.

## Prerequisites

- Mac mini running macOS (Apple Silicon or Intel)
- Admin access to the `pulseai-labs/PulseDB` GitHub repo
- The Mac mini should be always-on (or wake-on-LAN) if you want QA to run on every PR

---

## Step 1: Register the Runner on GitHub

1. Go to: https://github.com/pulseai-labs/PulseDB/settings/actions/runners
2. Click **"New self-hosted runner"** → select **macOS** → select your architecture (ARM64 for Apple Silicon, x64 for Intel)
3. GitHub shows a registration token + a series of commands. **Copy the token** — you'll need it below.

## Step 2: Install the Runner on the Mac Mini

On the Mac mini, open Terminal:

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

# Install as a launchd service (starts on boot)
sudo ./svc.sh install

# Start the service
sudo ./svc.sh start
```

Verify it's running:
```bash
sudo ./svc.sh status
```

The runner should show as "Idle" on https://github.com/pulseai-labs/PulseDB/settings/actions/runners

---

## Step 3: Install Rust Toolchain

The QA workflow compiles + runs Rust test programs. The Mac mini needs Rust:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustc --version  # should show 1.89+
```

---

## Step 4: Install Droid CLI

```bash
curl -fsSL https://app.factory.ai/cli | sh
```

## Step 5: Authenticate Droid

```bash
droid login
```

This opens a browser for OAuth. Log in with your Factory account. Verify:

```bash
droid auth status
```

## Step 6: Configure the GLM5.2 Custom Model

The custom model config lives in `~/.factory/settings.json`. Copy your existing config
from your main Mac. The key section is `customModels` + `sessionDefaultSettings`:

```bash
# Edit the settings file
nano ~/.factory/settings.json
```

Add (or copy from your main Mac's `~/.factory/settings.json`):

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

If this prints a response, the setup is complete.

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

---

## What the Workflow Does on the Runner

When a PR is opened or pushed:

1. Checks out the PR branch code
2. Runs `droid exec --auto high -m "custom:GLM-[Z.AI-Coding-Plan]---Openai-0"` with the QA skill prompt
3. Droid reads the git diff, writes consumer-simulation Rust test programs, compiles + runs them
4. Posts the QA report as a PR comment
5. Uploads artifacts (report + output)

The runner uses the Mac mini's local Droid auth + GLM model config — **no GitHub secrets needed**.
