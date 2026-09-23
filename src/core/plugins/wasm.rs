//! WASM 插件沙箱
//!
//! 阶段三新增（spec §2 [S3]）。
//!
//! 使用 wasmtime 编译并实例化 WASM 插件，在受限的线性内存中执行。
//!
//! 插件 ABI（极简约定）:
//!
//! - 可导出可选函数 `plugin_transform(ptr: i32, len: i32) -> i32`：
//!   接收一段从插件线性内存 `memory` 偏移 `ptr`、长度为 `len` 的 payload，
//!   就地改写后返回新的长度（0 表示无需透传 / 未处理）。未导出则该 hook 为空操作。
//! - 可导出可选函数 `plugin_phase(phase: i32) -> i32`：返回 0 表示成功，非 0 表示失败。
//!
//! 两种约定至少其一被识别即视为可调用插件；都没有导出时 hook 退化为 Ok。

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use wasmtime::{Config, Engine, Instance, Module, Store};

use super::r#trait::{Plugin, PluginError, PluginMetadata, ProxyContext, RequestContext};

/// 写入 payload 的线性内存偏移（避开数据段区域）
const PAYLOAD_OFFSET: usize = 1024;

// 请求生命周期阶段编码
const PHASE_ON_REQUEST: i32 = 1;
const PHASE_BEFORE_PROXY: i32 = 2;
const PHASE_AFTER_PROXY: i32 = 3;

/// WASM 插件的实例内部状态（wasmtime Store 需独占可变访问，用 Mutex 包裹）
struct WasmInner {
    store: Store<()>,
    instance: Instance,
}

/// WASM 插件加载器
///
/// 从 WASM 文件（.wasm / .wat）加载插件，在沙箱中执行。
pub struct WasmPluginLoader {
    engine: Engine,
}

impl WasmPluginLoader {
    /// 创建 WASM 插件加载器
    pub fn new() -> Result<Self, PluginError> {
        let mut config = Config::new();
        // 默认关闭大多数 WASI/特性，仅保留核心即时编译，降低攻击面
        config.wasm_component_model(false);
        config.consume_fuel(true); // 启用燃料限制，防止恶意重循环
        let engine = Engine::new(&config)
            .map_err(|e| PluginError::InitFailed(format!("init wasmtime engine: {e}")))?;
        Ok(Self { engine })
    }

    /// 从 WASM 文件加载插件（.wasm 字节 或 .wat 文本均可）
    pub fn load_from_wasm<P: AsRef<Path>>(&self, path: P) -> Result<Arc<dyn Plugin>, PluginError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| {
            PluginError::NotFound(format!("cannot read plugin file {}: {e}", path.display()))
        })?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "wasm-plugin".to_string());
        let module = Module::new(&self.engine, &bytes)
            .map_err(|e| PluginError::InitFailed(format!("compile {}: {e}", path.display())))?;
        WasmPlugin::assemble(self.engine.clone(), module, name)
            .map(|p| Arc::new(p) as Arc<dyn Plugin>)
    }

    /// 直接从未知来源的 WAT / wasm 字节编译加载
    pub fn from_bytes(&self, bytes: &[u8], name: &str) -> Result<Arc<dyn Plugin>, PluginError> {
        let module = Module::new(&self.engine, bytes)
            .map_err(|e| PluginError::InitFailed(format!("compile wat: {e}")))?;
        WasmPlugin::assemble(self.engine.clone(), module, name.to_string())
            .map(|p| Arc::new(p) as Arc<dyn Plugin>)
    }
}

impl Default for WasmPluginLoader {
    fn default() -> Self {
        Self::new().expect("failed to create wasmtime engine")
    }
}

/// WASM 编译产出的插件实例
///
/// 每个实例持有独立的 Store<()>，因此不是 Clone，需用 Box/Arc 传递。
pub struct WasmPlugin {
    name: String,
    inner: Mutex<WasmInner>,
}

impl WasmPlugin {
    fn assemble(engine: Engine, module: Module, name: String) -> Result<Self, PluginError> {
        // 注入一个最小规模的线性内存（若插件未导出 memory，则忽略）
        let mut store = Store::new(&engine, ());
        // 预填充燃料预算，防止 consume_fuel 下插件一调用就触发 out-of-gas 中止
        store
            .set_fuel(10_000_000)
            .map_err(|_| PluginError::InitFailed("failed to set wasm fuel budget".to_string()))?;
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| PluginError::InitFailed(format!("instantiate '{}': {e}", name)))?;
        Ok(Self {
            name,
            inner: Mutex::new(WasmInner { store, instance }),
        })
    }

    /// 调用可选导出的 `plugin_transform(ptr, len) -> i32`，把 payload 写进线性内存后执行。
    fn call_transform(&self, payload: &[u8]) -> Result<i32, PluginError> {
        let mut guard = self.inner.lock().map_err(|_| {
            PluginError::ExecutionFailed("wasm plugin store lock poisoned".to_string())
        })?;
        let inner = &mut *guard;
        let memory = match inner.instance.get_memory(&mut inner.store, "memory") {
            Some(m) => m,
            None => {
                // 未导出 memory 的模块无法接收 payload，视为无 transform
                return Ok(0);
            }
        };

        let func = match inner
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut inner.store, "plugin_transform")
        {
            Ok(f) => f,
            Err(_) => {
                // 未导出 plugin_transform，回退到 plugin_phase 语义（见 call_phase）
                return Ok(0);
            }
        };

        // 确保线性内存容量足够
        const WASM_PAGE_SIZE: usize = 65536;
        let needed = PAYLOAD_OFFSET + payload.len();
        let cur = memory.data_size(&mut inner.store);
        if needed > cur {
            let pages = (needed - cur).div_ceil(WASM_PAGE_SIZE) as u64;
            memory
                .grow(&mut inner.store, pages)
                .map_err(|e| PluginError::ExecutionFailed(format!("grow wasm memory: {e}")))?;
        }

        // 写入 payload
        let data = memory.data_mut(&mut inner.store);
        data[PAYLOAD_OFFSET..PAYLOAD_OFFSET + payload.len()].copy_from_slice(payload);

        let new_len = func
            .call(
                &mut inner.store,
                (PAYLOAD_OFFSET as i32, payload.len() as i32),
            )
            .map_err(|e| PluginError::ExecutionFailed(format!("invoke plugin_transform: {e}")))?;
        Ok(new_len)
    }

    /// 调用可选导出的 `plugin_phase(phase) -> i32`
    fn call_phase(&self, phase: i32) -> Result<(), PluginError> {
        let mut guard = self.inner.lock().map_err(|_| {
            PluginError::ExecutionFailed("wasm plugin store lock poisoned".to_string())
        })?;
        let inner = &mut *guard;
        let func = match inner
            .instance
            .get_typed_func::<(i32,), i32>(&mut inner.store, "plugin_phase")
        {
            Ok(f) => f,
            Err(_) => return Ok(()), // 未导出，空操作
        };
        let code = func
            .call(&mut inner.store, (phase,))
            .map_err(|e| PluginError::ExecutionFailed(format!("invoke plugin_phase: {e}")))?;
        if code == 0 {
            Ok(())
        } else {
            Err(PluginError::ExecutionFailed(format!(
                "plugin_phase({phase}) returned {code}"
            )))
        }
    }
}

impl WasmPlugin {
    /// 序列化 payload 上下文，供 transform 调用使用
    fn body_bytes(value: &Option<Value>) -> Vec<u8> {
        match value {
            Some(v) => serde_json::to_vec(v).unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

#[async_trait]
impl Plugin for WasmPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: self.name.clone(),
            version: "0.0.0".to_string(),
            author: None,
            description: Some("WASM plugin loaded via wasmtime".to_string()),
        }
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> Result<(), PluginError> {
        // 优先用 payload 透传，其次用 phase 钩子
        if self.instance_exports("plugin_transform") {
            let payload = Self::body_bytes(
                &ctx.body
                    .as_ref()
                    .map(|b| Value::from(String::from_utf8_lossy(b).to_string())),
            );
            let _ = self.call_transform(&payload)?;
            Ok(())
        } else {
            self.call_phase(PHASE_ON_REQUEST)
        }
    }

    async fn before_proxy(&self, ctx: &mut ProxyContext) -> Result<(), PluginError> {
        if self.instance_exports("plugin_transform") {
            let payload = Self::body_bytes(
                &ctx.body
                    .as_ref()
                    .map(|b| Value::from(String::from_utf8_lossy(b).to_string())),
            );
            let _ = self.call_transform(&payload)?;
            Ok(())
        } else {
            self.call_phase(PHASE_BEFORE_PROXY)
        }
    }

    async fn after_proxy(&self, ctx: &mut ProxyContext) -> Result<(), PluginError> {
        if self.instance_exports("plugin_transform") {
            let payload = Self::body_bytes(
                &ctx.body
                    .as_ref()
                    .map(|b| Value::from(String::from_utf8_lossy(b).to_string())),
            );
            let _ = self.call_transform(&payload)?;
            Ok(())
        } else {
            self.call_phase(PHASE_AFTER_PROXY)
        }
    }
}

impl WasmPlugin {
    fn instance_exports(&self, name: &str) -> bool {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        let inner = &mut *guard;
        inner.instance.get_func(&mut inner.store, name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个真实可实例化的 WAT 模块：导出 memory + plugin_transform，
    /// 接收 (ptr, len)，返回 0 表示已处理（读取了 payload 长度）。
    const WAT_TRANSFORM: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "plugin_transform") (param $ptr i32) (param $len i32) (result i32)
            ;; 读一下 payload 长度并返回 0，表示成功处理
            local.get $len)
        )
    "#;

    /// 导出 plugin_phase 的 WAT 模块：
    /// `plugin_phase(phase) -> i32` 返回 `(phase == 2)` 的布尔值（0/1），
    /// 即 before_proxy（phase=2）时返回 1 表示拒绝，其余 phase 返回 0。
    const WAT_PHASE: &str = r#"
        (module
          (func (export "plugin_phase") (param $phase i32) (result i32)
            i32.const 2
            local.get $phase
            i32.eq)
        )
    "#;

    #[test]
    fn compile_and_call_transform() {
        let loader = WasmPluginLoader::new().unwrap();
        let plugin = loader
            .from_bytes(WAT_TRANSFORM.as_bytes(), "transformer")
            .unwrap();
        assert_eq!(plugin.metadata().name, "transformer");

        // transform 路径返回 Ok
        let mut pctx = ProxyContext {
            upstream_url: "http://upstream".into(),
            headers: Default::default(),
            body: Some(br#"{"role":"user"}"#.to_vec()),
            metadata: Default::default(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(plugin.before_proxy(&mut pctx)).unwrap();
    }

    #[tokio::test]
    async fn plugin_phase_hook_returns_error_for_before_proxy() {
        let loader = WasmPluginLoader::new().unwrap();
        let plugin = loader.from_bytes(WAT_PHASE.as_bytes(), "phaser").unwrap();
        let mut pctx = ProxyContext {
            upstream_url: "http://upstream".into(),
            headers: Default::default(),
            body: None,
            metadata: Default::default(),
        };
        // phase==2 (before_proxy) 返回 2，应报 ExecutionFailed
        let err = plugin.before_proxy(&mut pctx).await.unwrap_err();
        assert!(matches!(err, PluginError::ExecutionFailed(_)));
        // phase==1 (on_request) 返回 0，应 Ok
        let mut rctx = RequestContext {
            method: "POST".into(),
            path: "/v1/chat".into(),
            headers: Default::default(),
            query_params: Default::default(),
            body: None,
            metadata: Default::default(),
        };
        plugin.on_request(&mut rctx).await.unwrap();
    }

    #[test]
    fn missing_file_is_not_found() {
        let loader = WasmPluginLoader::new().unwrap();
        let result = loader.load_from_wasm("/nonexistent/plugin.wasm");
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }
}
