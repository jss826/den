param(
    [Parameter(Mandatory)]
    [string]$Password
)

$env:DEN_ENV = "production"
$env:DEN_PASSWORD = $Password
cargo run --release
