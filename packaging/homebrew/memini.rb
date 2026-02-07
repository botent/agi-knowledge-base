class Memini < Formula
  desc "Memini by AG\\I — interactive TUI for AI chat with persistent memory"
  homepage "https://github.com/botent/agi-knowledge-base"
  version "0.1.0"

  if Hardware::CPU.arm?
    url "https://github.com/botent/agi-knowledge-base/releases/download/v#{version}/memini-v#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "REPLACE_WITH_ARM64_SHA256"
  else
    url "https://github.com/botent/agi-knowledge-base/releases/download/v#{version}/memini-v#{version}-x86_64-apple-darwin.tar.gz"
    sha256 "REPLACE_WITH_X86_64_SHA256"
  end

  license "MIT"

  def install
    bin.install "memini"
  end

  test do
    assert_match "memini", shell_output("#{bin}/memini --help 2>&1", 1)
  end
end
