//! A resumable run owns its outputs, locks its workspace and binds markers to one immutable plan.
use crate::pipeline_files::{
    digest, inventory, library_identity, link, location, owned_inventory, pin_manifest,
    source_identity, stamp, write_json,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

pub struct Run {
    pub root: PathBuf,
    pub inputs: PathBuf,
    pub out: PathBuf,
    pub manifest: PathBuf,
    executable: PathBuf,
    source_year: PathBuf,
    source_identity: String,
    library_inputs: Vec<(PathBuf, String)>,
    lock: fs::File,
}

impl Run {
    #[cfg(test)]
    pub(crate) fn set_executable_for_test(&mut self, executable: &Path) {
        self.executable = executable.to_owned();
    }

    pub fn open(
        manifest: &Path,
        sources: &Path,
        out: &Path,
        root: &Path,
        selection: &str,
        options: &serde_json::Value,
    ) -> Result<Self> {
        let root = location(root)?;
        let manifest = manifest.canonicalize()?;
        let sources = sources.canonicalize()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let definition = crate::year::Manifest::read(&manifest).map_err(anyhow::Error::msg)?;
        crate::pipeline_plan::check_generated_aliases(&definition, &sources)?;
        let year = definition.year;
        ensure!(!out.is_symlink(), "year output must not be a symlink");
        let out = location(out)?;
        ensure!(
            !root.starts_with(&out) && !out.starts_with(&root),
            "year output and scratch must be disjoint"
        );
        ensure!(
            !sources.starts_with(&out)
                && !out.starts_with(&sources)
                && !root.starts_with(&sources)
                && !sources.starts_with(&root),
            "inputs, output and scratch must be disjoint"
        );
        fs::create_dir_all(&root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("lock"))?;
        lock.try_lock()
            .context("another year build owns this scratch directory")?;
        let source_year = sources.join(&year);
        let source_identity = source_identity(&source_year)?;
        let library_inputs = options["road-library-path"]
            .as_str()
            .map(std::ffi::OsString::from)
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .map(|path| {
                let path = path.canonicalize()?;
                Ok((path.clone(), library_identity(&path)?))
            })
            .collect::<Result<Vec<_>>>()?;
        let identity = json!({
            "manifest_sha256": digest(&manifest)?,
            "builder_sha256": digest(&executable)?,
            "source_inventory_sha256": source_identity,
            "options": options,
            "library_path": std::env::var_os("LD_LIBRARY_PATH"),
            "library_inputs": library_inputs,
            "sources": sources, "out": out, "squares": selection,
        });
        let record = root.join("run.json");
        if record.exists() {
            ensure!(
                serde_json::from_slice::<serde_json::Value>(&fs::read(&record)?)? == identity,
                "run inputs, builder or selection changed; use a new scratch and output directory"
            );
        } else {
            ensure!(
                !out.exists(),
                "a new year run requires a new output directory"
            );
            ensure!(
                fs::read_dir(&root)?.count() == 1,
                "a new year run requires empty scratch"
            );
            fs::create_dir_all(out.parent().context("output needs a parent")?)?;
            fs::create_dir(&out).context("another run claimed the year output")?;
            write_json(&record, &identity)?;
        }
        let pinned_builder = root.join("qm-build");
        if !pinned_builder.exists() {
            let temporary = root.join("qm-build.tmp");
            fs::copy(&executable, &temporary)?;
            fs::rename(temporary, &pinned_builder)?;
        }
        ensure!(
            json!(digest(&pinned_builder)?) == identity["builder_sha256"],
            "pinned builder changed"
        );
        let manifest = pin_manifest(&root, &manifest, &year)?;
        fs::create_dir_all(&out)?;
        let inputs = root.join("inputs");
        let input_year = inputs.join(&year);
        fs::create_dir_all(&input_year)?;
        for entry in fs::read_dir(sources.join(&year))? {
            let entry = entry?;
            link(&entry.path(), &input_year.join(entry.file_name()))?;
        }
        fs::create_dir_all(root.join("steps"))?;
        Ok(Self {
            root,
            inputs,
            out,
            manifest,
            executable: pinned_builder,
            source_year,
            source_identity,
            library_inputs,
            lock,
        })
    }

    pub fn stage_root(&self, name: &str) -> PathBuf {
        self.root.join("steps").join(name)
    }

    pub fn verify_inputs(&self) -> Result<()> {
        ensure!(
            source_identity(&self.source_year)? == self.source_identity,
            "publisher inputs changed during the build"
        );
        for (path, identity) in &self.library_inputs {
            ensure!(
                library_identity(path)? == *identity,
                "runtime libraries changed during the build"
            );
        }
        Ok(())
    }

    pub(crate) fn child_command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        let descriptor = self.lock.as_raw_fd();
        // A surviving preparer must retain ownership if its runner is killed.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(descriptor, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
    }

    pub fn execute(
        &self,
        name: &str,
        output: &Path,
        extra: &[(&str, String)],
        library_path: Option<&str>,
    ) -> Result<()> {
        let root = self.stage_root(name);
        let marker = root.join("complete.json");
        if marker.exists() {
            let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&marker)?)?;
            for file in receipt["files"]
                .as_array()
                .context("invalid completion marker")?
            {
                let path = Path::new(file[0].as_str().context("invalid completed file")?);
                ensure!(
                    stamp(path)? == *file,
                    "completed output changed: {}",
                    path.display()
                );
            }
            eprintln!("year: {name}: complete");
            return Ok(());
        }
        // Atomic raster files, native progress and aircraft days belong to this immutable run.
        if root.exists()
            && !matches!(
                name,
                "prepare-rasters"
                    | "traffic"
                    | "aircraft-segments"
                    | "aircraft-shuffle"
                    | "aircraft-boxes"
                    | "aircraft-events"
                    | "aircraft-tracks"
            )
        {
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if entry.file_name() != "build.log" {
                    if entry.file_type()?.is_dir() {
                        fs::remove_dir_all(entry.path())?;
                    } else {
                        fs::remove_file(entry.path())?;
                    }
                }
            }
        }
        fs::create_dir_all(&root)?;
        let started = Instant::now();
        let cpu_before = children_cpu_seconds();
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("build.log"))?;
        let mut child = self.child_command();
        child
            .arg(name)
            .arg("--year")
            .arg(&self.manifest)
            .arg("--sources")
            .arg(&self.inputs)
            .arg("--out")
            .arg(output)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        for (key, value) in extra {
            child.arg(format!("--{key}")).arg(value);
        }
        if library_path.is_some() {
            child.env(
                "LD_LIBRARY_PATH",
                std::env::join_paths(self.library_inputs.iter().map(|(path, _)| path))?,
            );
        }
        eprintln!(
            "year: {name}: starting; {}",
            root.join("build.log").display()
        );
        let status = child.status()?;
        ensure!(
            status.success(),
            "{name}: {status}; see {}",
            root.join("build.log").display()
        );
        self.verify_inputs()?;
        let mut files = Vec::new();
        owned_inventory(output, name, &mut files)?;
        for family in if name == "osm" {
            &["railway-ways", "train-routes"][..]
        } else {
            &[]
        } {
            let sibling =
                crate::osm::year_sibling_path(output, family).map_err(anyhow::Error::msg)?;
            if sibling.exists() {
                inventory(&sibling, &mut files)?;
            }
        }
        let seconds = started.elapsed().as_secs_f64();
        let cpu_seconds = children_cpu_seconds() - cpu_before;
        write_json(
            &marker,
            &json!({"seconds": seconds, "cpu_seconds": cpu_seconds, "files": files}),
        )?;
        // A step that sits on one core for long shows here already on a small selection, before a
        // world run multiplies it.
        eprintln!(
            "year: {name}: {seconds:.3} seconds on {:.1} cores",
            cpu_seconds / seconds
        );
        Ok(())
    }
}

/// User and system seconds of the finished children this process has waited for.
fn children_cpu_seconds() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the rusage it is given; RUSAGE_CHILDREN is always valid.
    let usage = unsafe {
        libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr());
        usage.assume_init()
    };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}
