//
// Copyright (c) 2023 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//
use crate::*;
use libloading::Library;
use std::{
    borrow::Cow,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use vtable::{Compatibility, PluginLoaderVersion, PluginVTable, PLUGIN_LOADER_VERSION};
use zenoh_result::{bail, ZResult};
use zenoh_util::LibLoader;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PluginState {
    Declared,
    Loaded,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginCondition {
    warnings: Vec<Cow<'static, str>>,
    errors: Vec<Cow<'static, str>>,
}

impl PluginCondition {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&mut self) {
        self.warnings.clear();
        self.errors.clear();
    }
    pub fn add_error<S: Into<Cow<'static, str>>>(&mut self, error: S) {
        self.errors.push(error.into());
    }
    pub fn add_warning<S: Into<Cow<'static, str>>>(&mut self, warning: S) {
        self.warnings.push(warning.into());
    }
    pub fn catch_error<T, F: FnOnce() -> ZResult<T>>(&mut self, f: F) -> ZResult<T> {
        self.clear();
        match f() {
            Ok(v) => Ok(v),
            Err(e) => {
                self.add_error(format!("{}", e));
                Err(e)
            }
        }
    }
    pub fn errors(&self) -> &[Cow<'static, str>] {
        &self.errors
    }
    pub fn warnings(&self) -> &[Cow<'static, str>] {
        &self.warnings
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginStatus {
    pub state: PluginState,
    pub condition: PluginCondition,
}

pub trait PluginInfo {
    fn name(&self) -> &str;
    fn path(&self) -> &str;
    fn status(&self) -> PluginStatus;
}

pub trait DeclaredPlugin<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>:
    PluginInfo
{
    fn load(&mut self) -> ZResult<&mut dyn LoadedPlugin<StartArgs, Instance>>;
    fn loaded(&self) -> Option<&dyn LoadedPlugin<StartArgs, Instance>>;
    fn loaded_mut(&mut self) -> Option<&mut dyn LoadedPlugin<StartArgs, Instance>>;
}
pub trait LoadedPlugin<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>:
    PluginInfo
{
    fn run(&mut self, args: &StartArgs) -> ZResult<&mut dyn RunningPlugin<StartArgs, Instance>>;
    fn running(&self) -> Option<&dyn RunningPlugin<StartArgs, Instance>>;
    fn running_mut(&mut self) -> Option<&mut dyn RunningPlugin<StartArgs, Instance>>;
}

pub trait RunningPlugin<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion> {
    fn stop(&mut self);
    fn instance(&self) -> &Instance;
    fn instance_mut(&mut self) -> &mut Instance;
}

struct StaticPlugin<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    instance: Option<Instance>,
    phantom: PhantomData<P>,
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P>
    StaticPlugin<StartArgs, Instance, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    fn new() -> Self {
        Self {
            instance: None,
            phantom: PhantomData,
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P> PluginInfo
    for StaticPlugin<StartArgs, Instance, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    fn name(&self) -> &str {
        P::STATIC_NAME
    }
    fn path(&self) -> &str {
        "<static>"
    }
    fn status(&self) -> PluginStatus {
        PluginStatus {
            state: self
                .instance
                .map_or(PluginState::Loaded, |_| PluginState::Running),
            condition: PluginCondition::new(), // TODO: request runnnig plugin status
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P>
    DeclaredPlugin<StartArgs, Instance> for StaticPlugin<StartArgs, Instance, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    fn load(&mut self) -> ZResult<&mut dyn LoadedPlugin<StartArgs, Instance>> {
        Ok(self)
    }
    fn loaded(&self) -> Option<&dyn LoadedPlugin<StartArgs, Instance>> {
        Some(self)
    }
    fn loaded_mut(&mut self) -> Option<&mut dyn LoadedPlugin<StartArgs, Instance>> {
        Some(self)
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P>
    LoadedPlugin<StartArgs, Instance> for StaticPlugin<StartArgs, Instance, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    fn run(&mut self, args: &StartArgs) -> ZResult<&mut dyn RunningPlugin<StartArgs, Instance>> {
        if self.instance.is_none() {
            self.instance = Some(P::start(self.name(), args)?);
        }
        Ok(self)
    }
    fn running(&self) -> Option<&dyn RunningPlugin<StartArgs, Instance>> {
        if self.instance.is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn running_mut(&mut self) -> Option<&mut dyn RunningPlugin<StartArgs, Instance>> {
        if self.instance.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion, P>
    RunningPlugin<StartArgs, Instance> for StaticPlugin<StartArgs, Instance, P>
where
    P: Plugin<StartArgs = StartArgs, Instance = Instance>,
{
    fn stop(&mut self) {}
    fn instance(&self) -> &Instance {
        self.instance.as_ref().unwrap()
    }
    fn instance_mut(&mut self) -> &mut Instance {
        self.instance.as_mut().unwrap()
    }
}

/// This enum contains information where to load the plugin from.
enum DynamicPluginSource {
    /// Load plugin with the name in String + `.so | .dll | .dylib`
    /// in LibLoader's search paths.
    ByName((LibLoader, String)),
    /// Load first avalilable plugin from the list of path to plugin files (absolute or relative to the current working directory)
    ByPaths(Vec<String>),
}

impl DynamicPluginSource {
    fn load(&self) -> ZResult<(Library, PathBuf)> {
        match self {
            DynamicPluginSource::ByName((libloader, name)) => unsafe {
                libloader.search_and_load(name)
            },
            DynamicPluginSource::ByPaths(paths) => {
                for path in paths {
                    match unsafe { LibLoader::load_file(path) } {
                        Ok((l, p)) => return Ok((l, p)),
                        Err(e) => log::warn!("Plugin {} load fail: {}", path, e),
                    }
                }
                bail!("Plugin not found in {:?}", &paths)
            }
        }
    }
}

struct DynamicPluginStarter<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion> {
    _lib: Library,
    path: PathBuf,
    vtable: PluginVTable<StartArgs, Instance>,
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>
    DynamicPluginStarter<StartArgs, Instance>
{
    fn get_vtable(lib: &Library, path: &Path) -> ZResult<PluginVTable<StartArgs, Instance>> {
        log::debug!("Loading plugin {}", &path.to_str().unwrap(),);
        let get_plugin_loader_version =
            unsafe { lib.get::<fn() -> PluginLoaderVersion>(b"get_plugin_loader_version")? };
        let plugin_loader_version = get_plugin_loader_version();
        log::debug!("Plugin loader version: {}", &plugin_loader_version);
        if plugin_loader_version != PLUGIN_LOADER_VERSION {
            bail!(
                "Plugin loader version mismatch: host = {}, plugin = {}",
                PLUGIN_LOADER_VERSION,
                plugin_loader_version
            );
        }
        let get_compatibility = unsafe { lib.get::<fn() -> Compatibility>(b"get_compatibility")? };
        let plugin_compatibility_record = get_compatibility();
        let host_compatibility_record = Compatibility::new::<StartArgs, Instance>();
        log::debug!(
            "Plugin compativilty record: {:?}",
            &plugin_compatibility_record
        );
        if !plugin_compatibility_record.are_compatible(&host_compatibility_record) {
            bail!(
                "Plugin compatibility mismatch:\n\nHost:\n{}\nPlugin:\n{}\n",
                host_compatibility_record,
                plugin_compatibility_record
            );
        }
        let load_plugin =
            unsafe { lib.get::<fn() -> PluginVTable<StartArgs, Instance>>(b"load_plugin")? };
        let vtable = load_plugin();
        Ok(vtable)
    }
    fn new(lib: Library, path: PathBuf) -> ZResult<Self> {
        let vtable = Self::get_vtable(&lib, &path)?;
        Ok(Self {
            _lib: lib,
            path,
            vtable,
        })
    }
    fn start(&self, name: &str, args: &StartArgs) -> ZResult<Instance> {
        (self.vtable.start)(name, args)
    }
    fn path(&self) -> &str {
        self.path.to_str().unwrap()
    }
}

struct DynamicPlugin<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion> {
    name: String,
    condition: PluginCondition,
    source: DynamicPluginSource,
    starter: Option<DynamicPluginStarter<StartArgs, Instance>>,
    instance: Option<Instance>,
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>
    DynamicPlugin<StartArgs, Instance>
{
    fn new(name: String, source: DynamicPluginSource) -> Self {
        Self {
            name,
            condition: PluginCondition::new(),
            source,
            starter: None,
            instance: None,
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion> PluginInfo
    for DynamicPlugin<StartArgs, Instance>
{
    fn name(&self) -> &str {
        self.name.as_str()
    }
    fn path(&self) -> &str {
        self.starter.as_ref().map_or("<not loaded>", |v| v.path())
    }
    fn status(&self) -> PluginStatus {
        PluginStatus {
            state: if self.starter.is_some() {
                if self.instance.is_some() {
                    PluginState::Running
                } else {
                    PluginState::Loaded
                }
            } else {
                PluginState::Declared
            },
            condition: self.condition.clone(), // TODO: request condition from running plugin
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>
    DeclaredPlugin<StartArgs, Instance> for DynamicPlugin<StartArgs, Instance>
{
    fn load(&mut self) -> ZResult<&mut dyn LoadedPlugin<StartArgs, Instance>> {
        if self.starter.is_none() {
            self.condition.catch_error(|| {
                let (lib, path) = self.source.load()?;
                self.starter = Some(DynamicPluginStarter::new(lib, path)?);
                Ok(())
            })?;
        }
        Ok(self)
    }
    fn loaded(&self) -> Option<&dyn LoadedPlugin<StartArgs, Instance>> {
        if self.starter.is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn loaded_mut(&mut self) -> Option<&mut dyn LoadedPlugin<StartArgs, Instance>> {
        if self.starter.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>
    LoadedPlugin<StartArgs, Instance> for DynamicPlugin<StartArgs, Instance>
{
    fn run(&mut self, args: &StartArgs) -> ZResult<&mut dyn RunningPlugin<StartArgs, Instance>> {
        self.condition.catch_error(|| {
            let starter = self
                .starter
                .as_ref()
                .ok_or_else(|| format!("Plugin `{}` not loaded", self.name))?;
            let already_running = self.instance.is_some();
            if !already_running {
                self.instance = Some(starter.start(self.name(), args)?);
            }
            Ok(())
        })?;
        Ok(self)
    }
    fn running(&self) -> Option<&dyn RunningPlugin<StartArgs, Instance>> {
        if self.instance.is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn running_mut(&mut self) -> Option<&mut dyn RunningPlugin<StartArgs, Instance>> {
        if self.instance.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion>
    RunningPlugin<StartArgs, Instance> for DynamicPlugin<StartArgs, Instance>
{
    fn stop(&mut self) {
        self.instance = None;
    }
    fn instance(&self) -> &Instance {
        self.instance.as_ref().unwrap()
    }
    fn instance_mut(&mut self) -> &mut Instance {
        self.instance.as_mut().unwrap()
    }
}

/// A plugins manager that handles starting and stopping plugins.
/// Plugins can be loaded from shared libraries using [`Self::load_plugin_by_name`] or [`Self::load_plugin_by_paths`], or added directly from the binary if available using [`Self::add_static`].
pub struct PluginsManager<StartArgs: CompatibilityVersion, Instance: CompatibilityVersion> {
    default_lib_prefix: String,
    loader: Option<LibLoader>,
    plugins: Vec<Box<dyn DeclaredPlugin<StartArgs, Instance>>>,
}

impl<StartArgs: 'static + CompatibilityVersion, Instance: 'static + CompatibilityVersion>
    PluginsManager<StartArgs, Instance>
{
    /// Constructs a new plugin manager with dynamic library loading enabled.
    pub fn dynamic<S: Into<String>>(loader: LibLoader, default_lib_prefix: S) -> Self {
        PluginsManager {
            default_lib_prefix: default_lib_prefix.into(),
            loader: Some(loader),
            plugins: Vec::new(),
        }
    }
    /// Constructs a new plugin manager with dynamic library loading disabled.
    pub fn static_plugins_only() -> Self {
        PluginsManager {
            default_lib_prefix: String::new(),
            loader: None,
            plugins: Vec::new(),
        }
    }

    /// Adds a statically linked plugin to the manager.
    pub fn add_static_plugin<
        P: Plugin<StartArgs = StartArgs, Instance = Instance> + Send + Sync,
    >(
        mut self,
    ) -> Self {
        let plugin_loader: StaticPlugin<StartArgs, Instance, P> = StaticPlugin::new();
        self.plugins.push(Box::new(plugin_loader));
        self
    }

    /// Add dynamic plugin to the manager by name, automatically prepending the default library prefix
    pub fn add_dynamic_plugin_by_name<S: Into<String>>(
        &mut self,
        name: S,
        plugin_name: &str,
    ) -> ZResult<&mut dyn DeclaredPlugin<StartArgs, Instance>> {
        let plugin_name = format!("{}{}", self.default_lib_prefix, plugin_name);
        let libloader = self
            .loader
            .as_ref()
            .ok_or("Dynamic plugin loading is disabled")?
            .clone();
        let loader = DynamicPlugin::new(
            plugin_name,
            DynamicPluginSource::ByName((libloader, plugin_name)),
        );
        self.plugins.push(Box::new(loader));
        let plugin = self.plugins.last_mut().unwrap();
        let plugin = plugin as &mut dyn DeclaredPlugin<StartArgs, Instance>;
        Ok(plugin)
    }

    /// Add first available dynamic plugin from the list of paths to the plugin files
    pub fn add_dynamic_plugin_by_paths<S: Into<String>, P: AsRef<str> + std::fmt::Debug>(
        &mut self,
        name: S,
        paths: &[P],
    ) -> ZResult<&mut dyn DeclaredPlugin<StartArgs, Instance>> {
        let name = name.into();
        let paths = paths.iter().map(|p| p.as_ref().into()).collect();
        let loader = DynamicPlugin::new(name, DynamicPluginSource::ByPaths(paths));
        self.plugins.push(Box::new(loader));
        let plugin = self.plugins.last_mut().unwrap();
        let plugin = plugin as &mut dyn DeclaredPlugin<StartArgs, Instance>;
        Ok(plugin)
    }

    fn get_plugin_index(&self, name: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.name() == name)
    }

    fn get_plugin_index_err(&self, name: &str) -> ZResult<usize> {
        self.get_plugin_index(name)
            .ok_or_else(|| format!("Plugin `{}` not found", name).into())
    }

    /// Lists all plugins
    pub fn plugins(&self) -> impl Iterator<Item = &dyn DeclaredPlugin<StartArgs, Instance>> + '_ {
        self.plugins.iter().map(|p| p.as_ref())
    }

    /// Lists all plugins mutable
    pub fn plugins_mut<'a>(
        &'a mut self,
    ) -> impl Iterator<Item = &'a mut (dyn DeclaredPlugin<StartArgs, Instance>+'a )> + 'a {
        self.plugins.iter_mut().map(move|p| p.as_mut())
    }

    /// Lists the loaded plugins
    pub fn loaded_plugins(
        &self,
    ) -> impl Iterator<Item = &dyn LoadedPlugin<StartArgs, Instance>> + '_ {
        self.plugins().filter_map(|p| p.loaded())
    }

    /// Lists the loaded plugins mutable
    pub fn loaded_plugins_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut dyn LoadedPlugin<StartArgs, Instance>> + '_ {
        self.plugins_mut().filter_map(|p| p.loaded_mut())
    }

    /// Lists the running plugins
    pub fn running_plugins(
        &self,
    ) -> impl Iterator<Item = &dyn RunningPlugin<StartArgs, Instance>> + '_ {
        self.loaded_plugins().filter_map(|p| p.running())
    }

    /// Lists the running plugins mutable
    pub fn running_plugins_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut dyn RunningPlugin<StartArgs, Instance>> + '_ {
        self.loaded_plugins_mut().filter_map(|p| p.running_mut())
    }

    /// Returns single plugin record
    pub fn plugin(&self, name: &str) -> ZResult<&dyn DeclaredPlugin<StartArgs, Instance>> {
        Ok(&self.plugins[self.get_plugin_index_err(name)?])
    }

    /// Returns mutable plugin record
    pub fn plugin_mut(
        &mut self,
        name: &str,
    ) -> ZResult<&mut dyn DeclaredPlugin<StartArgs, Instance>> {
        let index = self.get_plugin_index_err(name)?;
        Ok(&mut self.plugins[index])
    }

    /// Returns loaded plugin record
    pub fn loaded_plugin(&self, name: &str) -> ZResult<&dyn LoadedPlugin<StartArgs, Instance>> {
        Ok(self
            .plugin(name)?
            .loaded()
            .ok_or_else(|| format!("Plugin `{}` not loaded", name))?)
    }

    /// Returns mutable loaded plugin record
    pub fn loaded_plugin_mut(
        &mut self,
        name: &str,
    ) -> ZResult<&mut dyn LoadedPlugin<StartArgs, Instance>> {
        Ok(self
            .plugin_mut(name)?
            .loaded_mut()
            .ok_or_else(|| format!("Plugin `{}` not loaded", name))?)
    }

    /// Returns running plugin record
    pub fn running_plugin(&self, name: &str) -> ZResult<&dyn RunningPlugin<StartArgs, Instance>> {
        Ok(self
            .loaded_plugin(name)?
            .running()
            .ok_or_else(|| format!("Plugin `{}` is not running", name))?)
    }

    /// Returns mutable running plugin record
    pub fn running_plugin_mut(
        &mut self,
        name: &str,
    ) -> ZResult<&mut dyn RunningPlugin<StartArgs, Instance>> {
        Ok(self
            .loaded_plugin_mut(name)?
            .running_mut()
            .ok_or_else(|| format!("Plugin `{}` is not running", name))?)
    }
}
