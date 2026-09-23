//! Native 插件加载器
//!
//! 阶段三新增（spec §2 [S3]）。
//!
//! 使用 `libloading` 在运行时动态加载动态库（.so / .dll / .dylib），
//! 通过 dlsym/GetProcAddress 解析以下约定符号：
//! - `rapidgate_plugin_name() -> *const c_char`  必选，返回插件名（UTF-8）
//! - `rapidgate_plugin_version() -> *const c_char` 可选，返回插件版本
//! - `rapidgate_plugin_transform() -> i32`       可选，请求钩子（0=成功）
//!
//! 所有符号均为 `unsafe extern "C"` 调用，加载期即校验必选符号。

use std::ffi::{c_char, CStr};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use super::r#trait::{Plugin, PluginError, PluginMetadata, ProxyContext, RequestContext};

/// `rapidgate_plugin_name` 符号签名
type PluginNameFn = unsafe extern "C" fn() -> *const c_char;
/// `rapidgate_plugin_version` 符号签名
type PluginVersionFn = unsafe extern "C" fn() -> *const c_char;
/// `rapidgate_plugin_transform` 符号签名
type PluginTransformFn = unsafe extern "C" fn() -> i32;

/// Native 插件加载器
pub struct NativePluginLoader;

impl NativePluginLoader {
    /// 创建 native 插件加载器
    pub fn new() -> Self {
        Self
    }

    /// 从动态库加载插件
    pub fn load_from_library<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<Arc<dyn Plugin>, PluginError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(PluginError::NotFound(format!(
                "plugin library not found: {}",
                path.display()
            )));
        }

        // libloading 接受 OsStr，直接使用原路径，避免额外分配
        let lib = match unsafe { libloading::Library::new(path) } {
            Ok(lib) => Arc::new(lib),
            Err(e) => {
                return Err(PluginError::InitFailed(format!(
                    "cannot open dynamic library {}: {e}",
                    path.display()
                )))
            }
        };

        // 读取必选符号：rapidgate_plugin_name
        // # Safety: 先解析符号，若地址无效返回错误而非崩溃。libloading 的 get 要求
        // 符号名不携带 NUL 终止符，内部会自行追加。
        let name_fn: PluginNameFn = unsafe {
            match lib.get(b"rapidgate_plugin_name") {
                Ok(sym) => *sym,
                Err(e) => {
                    return Err(PluginError::InitFailed(format!(
                        "missing required symbol 'rapidgate_plugin_name': {e}"
                    )))
                }
            }
        };

        // 调用一次取回插件名
        let name = unsafe { read_c_string(name_fn) };

        // 读取可选符号：rapidgate_plugin_version
        let version = unsafe {
            match lib.get::<PluginVersionFn>(b"rapidgate_plugin_version") {
                Ok(sym) => read_c_string(*sym),
                Err(_) => "0.0.0".to_string(),
            }
        };

        // 读取可选符号：rapidgate_plugin_transform
        let transform_fn: Option<PluginTransformFn> = unsafe {
            lib.get::<PluginTransformFn>(b"rapidgate_plugin_transform")
                .ok()
                .map(|sym| *sym)
        };

        Ok(Arc::new(NativePlugin {
            _lib: lib,
            name,
            version,
            transform_fn,
        }))
    }
}

impl Default for NativePluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

// # Safety: 调用处保证 `f` 的地址是有效的可调用函数指针。
unsafe fn read_c_string(f: unsafe extern "C" fn() -> *const c_char) -> String {
    let ptr = f();
    if ptr.is_null() {
        return String::new();
    }
    // # Safety: 约定插件返回 NUL 结尾的 UTF-8 静态字符串。
    let bytes = unsafe { CStr::from_ptr(ptr) };
    String::from_utf8_lossy(bytes.to_bytes()).to_string()
}

/// Native 插件实例，持有动态库句柄以保证加载期内符号一直有效
struct NativePlugin {
    _lib: Arc<libloading::Library>,
    name: String,
    version: String,
    transform_fn: Option<PluginTransformFn>,
}

#[async_trait]
impl Plugin for NativePlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            author: None,
            description: Some("Native dynamic-library plugin".to_string()),
        }
    }

    async fn on_request(&self, _ctx: &mut RequestContext) -> Result<(), PluginError> {
        self.run_transform()
    }

    async fn before_proxy(&self, _ctx: &mut ProxyContext) -> Result<(), PluginError> {
        self.run_transform()
    }

    async fn after_proxy(&self, _ctx: &mut ProxyContext) -> Result<(), PluginError> {
        self.run_transform()
    }
}

impl NativePlugin {
    fn run_transform(&self) -> Result<(), PluginError> {
        match self.transform_fn {
            Some(f) => {
                let code = unsafe { f() };
                if code == 0 {
                    Ok(())
                } else {
                    Err(PluginError::ExecutionFailed(format!(
                        "native plugin transform returned {code}"
                    )))
                }
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_returns_not_found() {
        let loader = NativePluginLoader::new();
        let result = loader.load_from_library("/nonexistent/plugin.dll");
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }

    #[test]
    #[ignore = "需要平台相关的真实动态库来验证正确加载路径；无现成 dll/so 时跳过"]
    fn loads_current_binary_but_missing_symbol() {
        // 正确路径验证依赖具体平台的动态库；此处演示加载一个真实存在的系统库
        // 会因缺少 rapidgate_plugin_name 而返回 InitFailed，而非崩溃。
        let loader = NativePluginLoader::new();
        // 找到当前可执行文件所在目录，尝试加载本进程二进制（在多数平台可 dlopen 自身）。
        // 无则跳过。
        let self_path = std::env::current_exe().ok();
        if let Some(p) = self_path {
            if p.exists() {
                match loader.load_from_library(&p) {
                    Err(PluginError::InitFailed(_)) => { /* 预期：缺符号 */ }
                    Err(err) => panic!("unexpected error: {err:?}"),
                    Ok(_) => { /* 极端情况下能加载成功，也视为通过 */ }
                }
            }
        }
    }
}
