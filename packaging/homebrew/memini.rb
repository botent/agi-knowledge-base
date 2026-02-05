class Memini < Formula
  desc "Memini MCP TUI"
  homepage "https://github.com/<org>/<repo>"
  version "0.1.0"
  url "https://github.com/<org>/<repo>/releases/download/v#{version}/memini-#{version}-x86_64-apple-darwin.tar.gz"
  sha256 "REPLACE_WITH_SHA256"
  def install
    bin.install "memini"
  end

  test do
    assert_match "memini", shell_output("#{bin}/memini --help", 1)
  end
end
