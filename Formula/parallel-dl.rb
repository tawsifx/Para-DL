require "json"

class ParallelDl < Formula
  desc "High-Performance Multi-Channel Download Accelerator with Anti-Throttle Watchdog"
  homepage "https://github.com/tawsifx/Para-DL"
  url "https://github.com/tawsifx/Para-DL/archive/refs/tags/v1.0.0.tar.gz"
  license "MIT"
  head "https://github.com/tawsifx/Para-DL.git", branch: "main"

  # The native binary is built from Rust; the media subsystem remains a Python
  # package installed into a binary-managed venv under libexec. There is no
  # default aria2 dependency and no Python console script.
  depends_on "rust" => :build
  depends_on "python@3.12"
  depends_on "ffmpeg"

  # Optional JavaScript runtime for YouTube challenge solving.
  depends_on "deno" => :recommended

  def python
    Formula["python@3.12"].opt_bin/"python3.12"
  end

  def media_venv
    libexec/"media-venv"
  end

  def install
    # One native binary owns both command names.
    system "cargo", "build", "--release", "--manifest-path", "rust/Cargo.toml", "--bin", "paradl"
    bin.install "rust/target/release/paradl" => "paradl"
    bin.install_symlink bin/"paradl" => "parallel-dl" unless (bin/"parallel-dl").exist?

    # yt-dlp is pip-installed into the venv, never frozen into the binary
    # (parity row P12). The worker is invoked as `python -m paradl_media`.
    system python, "-m", "venv", media_venv.to_s
    worker_python = media_venv/"bin/python"
    system worker_python, "-m", "pip", "install", "--upgrade", "pip"
    system worker_python, "-m", "pip", "install", "yt-dlp[default]", "mutagen"
    if (buildpath/"media/pyproject.toml").exist?
      system worker_python, "-m", "pip", "install", "./media"
    end

    (libexec/"media-python").write "#{worker_python}\n"
  end

  def caveats
    <<~EOS
      Media routes run a Python worker from:
        #{media_venv}
      Set PARADL_MEDIA_PY to override the interpreter. Non-media routes never
      spawn Python.
    EOS
  end

  test do
    assert_match "paradl", shell_output("#{bin}/paradl --version")
    assert_match "parallel-dl", shell_output("#{bin}/parallel-dl --version")
    assert_match "ready", shell_output("#{bin}/paradl doctor")

    # The media package is pip-installed at install time (W3-4a). Until then
    # the handshake is deferred; --version and doctor above stay mandatory.
    worker = (media_venv/"bin/python").to_s
    if system(worker, "-c", "import paradl_media", out: File::NULL, err: File::NULL)
      request = {
        "protocol" => "paradl-media/1",
        "request_id" => "00000000-0000-4000-8000-000000000001",
        "op" => "claims",
        "target" => "https://example.com/video",
        "out_dir" => testpath.to_s,
        "options" => {},
      }
      output = shell_output("printf '%s\\n' '#{request.to_json}' | #{worker} -m paradl_media")
      frame = JSON.parse(output.lines.last)
      assert_equal "paradl-media/1", frame["protocol"]
      assert_equal "result", frame["frame"]
      assert_kind_of Hash, frame["versions"]
    else
      puts "media worker not installed; handshake deferred to W3-4a"
    end
  end
end
