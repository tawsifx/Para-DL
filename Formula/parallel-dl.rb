class ParallelDl < Formula
  include Language::Python::Virtualenv

  desc "High-Performance Multi-Channel Download Accelerator with Anti-Throttle Watchdog"
  homepage "https://github.com/tawsifx/pdl"
  url "https://github.com/tawsifx/pdl/archive/refs/tags/v1.0.0.tar.gz"
  license "MIT"
  head "https://github.com/tawsifx/pdl.git", branch: "main"

  depends_on "aria2"
  depends_on "ffmpeg"
  depends_on "python@3.12"

  # Optional runtime JavaScript engine for YouTube challenge solving
  depends_on "deno" => :recommended

  def install
    virtualenv_install_with_resources
    # Provide 'paradl' symlink alongside 'parallel-dl'
    bin.install_symlink bin/"parallel-dl" => "paradl" unless (bin/"paradl").exist?
  end

  test do
    assert_match "parallel-dl", shell_output("#{bin}/parallel-dl --version")
    assert_match "parallel-dl", shell_output("#{bin}/paradl --version")
    assert_match "ready", shell_output("#{bin}/paradl doctor")
  end
end
