#!/usr/bin/env bash
# Print the Homebrew formula for a released tag, using the .sha256 files from the GitHub release.
# Usage: scripts/brew-formula.sh v0.1.0 > Formula/lpt.rb   (run in a checkout of the tap)
set -euo pipefail
tag="${1:?usage: brew-formula.sh vX.Y.Z}"
repo="sevgjan/light-cli"
sha() { gh release download "$tag" -R "$repo" -p "lpt-$tag-$1.tar.gz.sha256" -O - | cut -d' ' -f1; }
url() { echo "https://github.com/$repo/releases/download/$tag/lpt-$tag-$1.tar.gz"; }
cat <<RB
class Lpt < Formula
  desc "Submit and manage Light Phone tools from the terminal (unofficial)"
  homepage "https://github.com/$repo"
  version "${tag#v}"
  license "MIT"

  on_macos do
    on_arm do
      url "$(url aarch64-apple-darwin)"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$(url x86_64-apple-darwin)"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$(url aarch64-unknown-linux-gnu)"
      sha256 "$(sha aarch64-unknown-linux-gnu)"
    end
    on_intel do
      url "$(url x86_64-unknown-linux-gnu)"
      sha256 "$(sha x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "lpt"
  end

  test do
    assert_match "lpt", shell_output("#{bin}/lpt --version")
  end
end
RB
