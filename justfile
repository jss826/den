set dotenv-load
set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

# Development build & run
dev:
    $env:DEN_DATA_DIR="./data-dev"; cargo run

# Production build & run
prod password="":
    if ("{{password}}") { $env:DEN_PASSWORD="{{password}}" }; $env:DEN_ENV="production"; cargo build --release; cargo run --release

# Hot reload development (requires: cargo install cargo-watch)
watch:
    $env:DEN_DATA_DIR="./data-dev"; cargo watch -x run

# Run all checks (fmt + clippy + test)
check:
    cargo fmt -- --check; if ($LASTEXITCODE -eq 0) { cargo clippy -- -D warnings }; if ($LASTEXITCODE -eq 0) { cargo test }

# Format code
fmt:
    cargo fmt

# Run tests only
test:
    cargo test

# E2E tests
e2e:
    npm run test:e2e

# Build only (no run)
build:
    cargo build

# List OpenConsole.exe processes (for diagnosing zombies)
ps:
    Get-Process -Name OpenConsole -ErrorAction SilentlyContinue | Format-Table Id, CPU, StartTime, MainWindowTitle -AutoSize; if (-not $?) { Write-Host "No OpenConsole processes found" }

# Clean build artifacts
clean:
    cargo clean
