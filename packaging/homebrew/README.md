# Homebrew Distribution

## How It Works

When you push a version tag (e.g. `v0.1.0`), the GitHub Actions release workflow:

1. Builds the `memini` binary for macOS (x86_64 + ARM64) and Linux (x86_64)
2. Packages each as a `.tar.gz` with SHA256 checksums
3. Creates a GitHub Release with all artifacts
4. Computes the SHA256 hashes and prints the updated formula

## Setting Up the Tap

Create a separate repo for the Homebrew tap:

```bash
# Create the tap repo at github.com/botent/homebrew-tap
# The repo name MUST start with "homebrew-" for `brew tap` to work
```

Add the formula file:

```
homebrew-tap/
└── Formula/
    └── memini.rb    # copy from packaging/homebrew/memini.rb
```

## Publishing a Release

```bash
# Tag and push
git tag v0.1.0
git push origin v0.1.0
```

After the workflow runs:

1. Go to the Actions tab and find the `update-homebrew` job output
2. Copy the SHA256 values
3. Update `Formula/memini.rb` in `botent/homebrew-tap` with the real SHA256 hashes
4. Commit and push to the tap repo

## Installing

Users can then install with:

```bash
brew tap botent/tap
brew install memini
```

## Updating the Formula Version

For each new release, the workflow outputs the updated SHA256 values. Update the formula in the tap repo:

1. Change `version` to the new version
2. Replace the SHA256 hashes with the new values from the workflow output
3. Push to `botent/homebrew-tap`

## Manual Build (Without Homebrew)

```bash
git clone https://github.com/botent/agi-knowledge-base
cd agi-knowledge-base
cargo build --release
# Binary at target/release/memini
```
