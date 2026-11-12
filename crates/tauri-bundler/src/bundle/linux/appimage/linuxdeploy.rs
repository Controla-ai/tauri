// Copyright 2016-2019 Cargo-Bundle developers <https://github.com/burtonageo/cargo-bundle>
// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use super::{super::debian, write_and_make_executable};
use crate::{
  Settings,
  bundle::settings::Arch,
  error::Context,
  utils::{
    CommandExt, fs_utils,
    http_utils::{HashAlgorithm, download_and_verify, verify_hash},
  },
};
use std::{
  fs,
  path::{Path, PathBuf},
  process::Command,
};

/// Bundles the project.
/// Returns a vector of PathBuf that shows where the AppImage was created.
pub fn bundle_project(settings: &Settings) -> crate::Result<Vec<PathBuf>> {
  // generate the deb binary name
  let appimage_arch: &str = match settings.binary_arch() {
    Arch::X86_64 => "amd64",
    Arch::X86 => "i386",
    Arch::AArch64 => "aarch64",
    Arch::Armhf => "armhf",
    target => {
      return Err(crate::Error::ArchError(format!(
        "Unsupported architecture: {target:?}"
      )));
    }
  };

  let tools_arch = if settings.binary_arch() == Arch::Armhf {
    "armhf"
  } else {
    settings.target().split('-').next().unwrap()
  };

  let output_path = settings.project_out_directory().join("bundle/appimage");
  if output_path.exists() {
    fs::remove_dir_all(&output_path)?;
  }

  let tools_path = settings
    .local_tools_directory()
    .map(|d| d.join(".tauri"))
    .unwrap_or_else(|| {
      dirs::cache_dir().map_or_else(|| output_path.to_path_buf(), |p| p.join("tauri"))
    });

  fs::create_dir_all(&tools_path)?;

  let tools = prepare_tools(&tools_path, tools_arch)?;

  let package_dir = settings.project_out_directory().join("bundle/appimage_deb");

  let main_binary = settings.main_binary()?;
  let product_name = settings.product_name();

  let mut settings = settings.clone();
  if main_binary.name().contains(' ') {
    let main_binary_path = settings.binary_path(main_binary);
    let project_out_directory = settings.project_out_directory();

    let main_binary_name_kebab = heck::AsKebabCase(main_binary.name()).to_string();
    let new_path = project_out_directory.join(&main_binary_name_kebab);
    fs::copy(main_binary_path, new_path)?;

    let main_binary = settings.main_binary_mut()?;
    main_binary.set_name(main_binary_name_kebab);
  }

  // generate deb_folder structure
  let (data_dir, icons) = debian::generate_data(&settings, &package_dir)
    .with_context(|| "Failed to build data folders and files")?;
  fs_utils::copy_custom_files(&settings.appimage().files, &data_dir)
    .with_context(|| "Failed to copy custom files")?;

  fs::create_dir_all(&output_path)?;
  let app_dir_path = output_path.join(format!("{}.AppDir", settings.product_name()));
  let appimage_filename = format!(
    "{}_{}_{}.AppImage",
    settings.product_name(),
    settings.version_string(),
    appimage_arch
  );
  let appimage_path = output_path.join(&appimage_filename);
  fs_utils::create_dir(&app_dir_path, true)?;

  fs::create_dir_all(&tools_path)?;
  let larger_icon = icons
    .iter()
    .filter(|i| i.width == i.height)
    .max_by_key(|i| i.width)
    .expect("couldn't find a square icon to use as AppImage icon");
  let larger_icon_path = larger_icon
    .path
    .strip_prefix(package_dir.join("data"))
    .unwrap()
    .to_string_lossy()
    .to_string();

  log::info!(action = "Bundling"; "{} ({})", appimage_filename, appimage_path.display());

  let app_dir_usr = app_dir_path.join("usr/");
  let app_dir_usr_bin = app_dir_usr.join("bin/");
  let app_dir_usr_lib = app_dir_usr.join("lib/");

  fs_utils::copy_dir(&data_dir.join("usr/"), &app_dir_usr)?;

  // Using create_dir_all for a single dir so we don't get errors if the path already exists
  fs::create_dir_all(&app_dir_usr_bin)?;
  fs::create_dir_all(app_dir_usr_lib)?;

  let search_dirs = [
    match settings.binary_arch() {
      Arch::X86_64 => "/usr/lib/x86_64-linux-gnu/",
      Arch::X86 => "/usr/lib/i386-linux-gnu/",
      Arch::AArch64 => "/usr/lib/aarch64-linux-gnu/",
      Arch::Armhf => "/usr/lib/arm-linux-gnueabihf/",
      _ => unreachable!(),
    },
    "/usr/lib64",
    "/usr/lib",
    "/usr/libexec",
  ];

  for file in [
    "WebKitNetworkProcess",
    "WebKitWebProcess",
    "injected-bundle/libwebkit2gtkinjectedbundle.so",
  ] {
    for source in search_dirs.map(PathBuf::from) {
      // TODO: Check if it's the same dir name on all systems
      let source = source.join("webkit2gtk-4.1").join(file);
      if source.exists() {
        fs_utils::copy_file(
          &source,
          &app_dir_path.join(source.strip_prefix("/").unwrap()),
        )?;
      }
    }
  }

  fs::copy(
    tools_path.join(format!("AppRun-{tools_arch}")),
    app_dir_path.join("AppRun"),
  )?;
  fs::copy(
    app_dir_path.join(larger_icon_path),
    app_dir_path.join(format!("{product_name}.png")),
  )?;
  std::os::unix::fs::symlink(format!("{product_name}.png"), app_dir_path.join(".DirIcon"))?;
  std::os::unix::fs::symlink(
    format!("usr/share/applications/{product_name}.desktop"),
    app_dir_path.join(format!("{product_name}.desktop")),
  )?;

  let log_level = match settings.log_level() {
    log::Level::Error => "3",
    log::Level::Warn => "2",
    log::Level::Info => "1",
    _ => "0",
  };

  let mut cmd = Command::new(&tools.linuxdeploy);
  cmd.env("OUTPUT", &appimage_path);
  if let Some(runtime) = &tools.type2_runtime {
    cmd.env("LDAI_RUNTIME_FILE", runtime);
  }
  cmd.env("ARCH", tools_arch);
  // Looks like the cli arg isn't enough for the updated AppImage output-plugin.
  cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
  cmd.args([
    "--appimage-extract-and-run",
    "--verbosity",
    log_level,
    "--appdir",
    &app_dir_path.display().to_string(),
    "--plugin",
    "gtk",
  ]);
  if settings.appimage().bundle_media_framework {
    cmd.args(["--plugin", "gstreamer"]);
  }
  cmd.args(["--output", "appimage"]);

  // Linuxdeploy logs everything into stderr so we have to ignore the output ourselves here
  if settings.log_level() == log::Level::Error {
    log::debug!(action = "Running"; "Command `linuxdeploy {}`", cmd.get_args().map(|arg| arg.to_string_lossy()).fold(String::new(), |acc, arg| format!("{acc} {arg}")));
    if !cmd.output()?.status.success() {
      return Err(crate::Error::GenericError(
        "failed to run linuxdeploy".to_string(),
      ));
    }
  } else {
    cmd.output_ok()?;
  }

  fs::remove_dir_all(&package_dir)?;
  Ok(vec![appimage_path])
}

const LINUXDEPLOY_COMMIT_HASH: &str = "07333c6";
const TYPE2_RUNTIME_RELEASE: &str = "20251108";

// SHA-256 of each downloaded tool, by `tools_arch`. Update them together with
// the release they are downloaded from.
const APPRUN_SHA256: &[(&str, &str)] = &[
  (
    "x86_64",
    "f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f",
  ),
  (
    "i686",
    "a573a682b1a4a3e9b5dddbd1f5785749b7bba6013149b51ae99d0f123fe11691",
  ),
  (
    "aarch64",
    "072f17c0895a85c490282fe5395c5007e5fc75da727e553b3b8fb680feb11578",
  ),
  (
    "armhf",
    "b14d89f0762bcf09fc6af2359d936675928b8833ff52a1aeda68cb28a309f6ba",
  ),
];
const LINUXDEPLOY_SHA256: &[(&str, &str)] = &[
  (
    "x86_64",
    "36a2d7e274d12e1050d0e9ecfe11d339ed54720b2bec464c286d53f8b07f5c62",
  ),
  (
    "i686",
    "fab919e3f88ec4460631f744ed5205bb7965ab4bc2c276227e12db81543afe9a",
  ),
  (
    "aarch64",
    "556ab80baa98e600aa80f0dcedfb70bca0e1ce7e9f147fb345be3fcc3e91b2b1",
  ),
  (
    "armhf",
    "2dad604a3b8110ff81f6f754d6fe1ba50b92bb75303385ab080966edddee28dd",
  ),
];
const TYPE2_RUNTIME_SHA256: &[(&str, &str)] = &[
  (
    "x86_64",
    "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
  ),
  (
    "i686",
    "e72ea0b140a0a16e680713238a6f30aad278b62c4ca17919c554864124515498",
  ),
  (
    "aarch64",
    "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
  ),
  (
    "armhf",
    "e9060d37577b8a29914ec12d8740add24e19ff29012fb1fa0f60daf62db0688d",
  ),
];

// linuxdeploy's AppImage magic bytes. They are zeroed in our copy so that
// AppImage integration tools don't pick it up.
const APPIMAGE_MAGIC_OFFSET: std::ops::Range<usize> = 8..11;
const APPIMAGE_MAGIC: &[u8] = b"AI\x02";

struct Tools {
  linuxdeploy: PathBuf,
  /// `None` when the user provides their own runtime through `LDAI_RUNTIME_FILE`.
  type2_runtime: Option<PathBuf>,
}

/// Returns a function that overwrites linuxdeploy's AppImage magic bytes.
fn set_magic(magic: &'static [u8]) -> impl Fn(&mut Vec<u8>) {
  move |data| {
    if let Some(bytes) = data.get_mut(APPIMAGE_MAGIC_OFFSET) {
      bytes.copy_from_slice(magic);
    }
  }
}

fn sha256_for(table: &[(&'static str, &'static str)], arch: &str) -> crate::Result<&'static str> {
  table
    .iter()
    .find(|(a, _)| *a == arch)
    .map(|(_, hash)| *hash)
    .ok_or_else(|| crate::Error::ArchError(format!("No AppImage tools for architecture {arch}")))
}

/// Writes `url` to `path` unless a file there already matches `sha256` after
/// `normalize`. Downloads are verified before `patch` runs and anything is written.
fn ensure_tool(
  path: &Path,
  url: &str,
  sha256: &str,
  normalize: impl Fn(&mut Vec<u8>),
  patch: impl Fn(&mut Vec<u8>),
) -> crate::Result<()> {
  if let Ok(mut cached) = fs::read(path) {
    normalize(&mut cached);
    if verify_hash(&cached, sha256, HashAlgorithm::Sha256).is_ok() {
      return Ok(());
    }
    log::warn!(
      "Cached {} doesn't match its expected hash. Downloading it again.",
      path.display()
    );
  }
  let mut data = download_and_verify(url, sha256, HashAlgorithm::Sha256)?;
  patch(&mut data);
  write_and_make_executable(path, &data)?;
  Ok(())
}

fn prepare_tools(tools_path: &Path, arch: &str) -> crate::Result<Tools> {
  ensure_tool(
    &tools_path.join(format!("AppRun-{arch}")),
    &format!(
      "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-{arch}"
    ),
    sha256_for(APPRUN_SHA256, arch)?,
    |_| {},
    |_| {},
  )?;

  let linuxdeploy_arch = if arch == "i686" { "i386" } else { arch };
  let linuxdeploy = tools_path.join(format!(
    "linuxdeploy-{LINUXDEPLOY_COMMIT_HASH}-{linuxdeploy_arch}.AppImage"
  ));
  ensure_tool(
    &linuxdeploy,
    &format!(
      "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-{LINUXDEPLOY_COMMIT_HASH}/linuxdeploy-{linuxdeploy_arch}.AppImage"
    ),
    sha256_for(LINUXDEPLOY_SHA256, arch)?,
    set_magic(APPIMAGE_MAGIC),
    set_magic(&[0; 3]),
  )?;

  // Older CLIs downloaded the AppImage output plugin next to linuxdeploy, which
  // then prefers it over its built-in plugin. Remove it so only verified tools run.
  let legacy_plugin = tools_path.join("linuxdeploy-plugin-appimage.AppImage");
  if legacy_plugin.exists() {
    fs::remove_file(&legacy_plugin)?;
  }

  // linuxdeploy's built-in AppImage plugin otherwise downloads the runtime
  // from the rolling `continuous` release. A runtime the user provides through
  // `LDAI_RUNTIME_FILE` is used as is.
  let type2_runtime = if std::env::var_os("LDAI_RUNTIME_FILE").is_some() {
    None
  } else {
    let path = tools_path.join(format!("runtime-{TYPE2_RUNTIME_RELEASE}-{arch}"));
    ensure_tool(
      &path,
      &format!(
        "https://github.com/AppImage/type2-runtime/releases/download/{TYPE2_RUNTIME_RELEASE}/runtime-{arch}"
      ),
      sha256_for(TYPE2_RUNTIME_SHA256, arch)?,
      |_| {},
      |_| {},
    )?;
    Some(path)
  };

  // The plugin scripts are embedded, so rewrite them whenever the cached copy differs:
  // a copy left behind by an older CLI would otherwise be used forever.
  for (name, data) in [
    (
      "linuxdeploy-plugin-gtk.sh",
      include_bytes!("./linuxdeploy-plugin-gtk.sh").as_slice(),
    ),
    (
      "linuxdeploy-plugin-gstreamer.sh",
      include_bytes!("./linuxdeploy-plugin-gstreamer.sh").as_slice(),
    ),
  ] {
    let path = tools_path.join(name);
    if fs::read(&path).ok().as_deref() != Some(data) {
      write_and_make_executable(&path, data)?;
    }
  }

  Ok(Tools {
    linuxdeploy,
    type2_runtime,
  })
}
