//! Android: UsbManager via JNI. Não exige root — o usuário autoriza o acesso
//! ao dispositivo USB num diálogo do sistema, e recebemos o descritor do usbfs.

use jni::JavaVM;
use jni::objects::{GlobalRef, JObject, JObjectArray, JString, JValue};
use jni::JNIEnv;
use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct UsbInfo {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub product: String,
    pub manufacturer: String,
}

type JRes<T> = jni::errors::Result<T>;

pub(crate) fn with_env<R>(f: impl FnOnce(&mut JNIEnv, &JObject) -> JRes<R>) -> io::Result<R> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| io::Error::other(format!("JNI: {e}")))?;
    let mut env = vm.attach_current_thread().map_err(|e| io::Error::other(format!("JNI: {e}")))?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    r.map_err(|e| io::Error::other(format!("JNI: {e}")))
}

fn manager<'a>(env: &mut JNIEnv<'a>, act: &JObject) -> JRes<JObject<'a>> {
    let svc = env.get_static_field("android/content/Context", "USB_SERVICE", "Ljava/lang/String;")?.l()?;
    env.call_method(act, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[JValue::Object(&svc)])?.l()
}

fn jstring(env: &mut JNIEnv, o: JObject) -> String {
    if o.is_null() {
        return String::new();
    }
    let s = JString::from(o);
    env.get_string(&s).map(|x| x.into()).unwrap_or_default()
}

/// Chama um getter de String tolerando SecurityException (ex.: sem permissão ainda).
fn safe_str(env: &mut JNIEnv, obj: &JObject, method: &str) -> String {
    match env.call_method(obj, method, "()Ljava/lang/String;", &[]).and_then(|v| v.l()) {
        Ok(o) => jstring(env, o),
        Err(_) => {
            let _ = env.exception_clear();
            String::new()
        }
    }
}

fn devices<'a>(env: &mut JNIEnv<'a>, mgr: &JObject) -> JRes<Vec<JObject<'a>>> {
    let map = env.call_method(mgr, "getDeviceList", "()Ljava/util/HashMap;", &[])?.l()?;
    let values = env.call_method(&map, "values", "()Ljava/util/Collection;", &[])?.l()?;
    let arr = JObjectArray::from(env.call_method(&values, "toArray", "()[Ljava/lang/Object;", &[])?.l()?);
    let n = env.get_array_length(&arr)?;
    let mut v = Vec::new();
    for i in 0..n {
        v.push(env.get_object_array_element(&arr, i)?);
    }
    Ok(v)
}

fn is_storage(env: &mut JNIEnv, dev: &JObject) -> JRes<bool> {
    let n = env.call_method(dev, "getInterfaceCount", "()I", &[])?.i()?;
    for i in 0..n {
        let intf = env.call_method(dev, "getInterface", "(I)Landroid/hardware/usb/UsbInterface;", &[JValue::Int(i)])?.l()?;
        if env.call_method(&intf, "getInterfaceClass", "()I", &[])?.i()? == 8 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Lista dispositivos USB de armazenamento conectados (não exige permissão).
pub fn list_usb_storage() -> Vec<UsbInfo> {
    with_env(|env, act| {
        let mgr = manager(env, act)?;
        let mut out = Vec::new();
        for d in devices(env, &mgr)? {
            if !is_storage(env, &d)? {
                continue;
            }
            let name = safe_str(env, &d, "getDeviceName");
            let vid = env.call_method(&d, "getVendorId", "()I", &[])?.i()? as u16;
            let pid = env.call_method(&d, "getProductId", "()I", &[])?.i()? as u16;
            let product = safe_str(env, &d, "getProductName");
            let manufacturer = safe_str(env, &d, "getManufacturerName");
            out.push(UsbInfo { name, vid, pid, product, manufacturer });
        }
        Ok(out)
    })
    .unwrap_or_default()
}

static CONNS: OnceLock<Mutex<HashMap<String, (GlobalRef, i32)>>> = OnceLock::new();

/// Pede permissão (diálogo do Android), abre o dispositivo e devolve o fd do usbfs.
/// A conexão Java fica guardada para o fd continuar válido.
pub fn open_device(name: &str) -> io::Result<i32> {
    let conns = CONNS.get_or_init(Default::default);
    if let Some((_, fd)) = conns.lock().unwrap().get(name) {
        return Ok(*fd);
    }
    let asked = with_env(|env, act| {
        let mgr = manager(env, act)?;
        let Some(dev) = find(env, &mgr, name)? else { return Ok(None) };
        let has = env.call_method(&mgr, "hasPermission", "(Landroid/hardware/usb/UsbDevice;)Z", &[JValue::Object(&dev)])?.z()?;
        if !has {
            let action = env.new_string("com.discolab.app.USB_PERMISSION")?;
            let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
            let pkg = env.call_method(act, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
            env.call_method(&intent, "setPackage", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&pkg)])?;
            const FLAG_IMMUTABLE: i32 = 0x0400_0000;
            let pi = env
                .call_static_method(
                    "android/app/PendingIntent",
                    "getBroadcast",
                    "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
                    &[JValue::Object(act), JValue::Int(0), JValue::Object(&intent), JValue::Int(FLAG_IMMUTABLE)],
                )?
                .l()?;
            env.call_method(
                &mgr,
                "requestPermission",
                "(Landroid/hardware/usb/UsbDevice;Landroid/app/PendingIntent;)V",
                &[JValue::Object(&dev), JValue::Object(&pi)],
            )?;
        }
        Ok(Some(has))
    })?;
    if asked.is_none() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "dispositivo USB desconectado"));
    }
    // Aguarda o usuário responder ao diálogo.
    let t0 = Instant::now();
    loop {
        let granted = with_env(|env, act| {
            let mgr = manager(env, act)?;
            let Some(dev) = find(env, &mgr, name)? else { return Ok(None) };
            if !env.call_method(&mgr, "hasPermission", "(Landroid/hardware/usb/UsbDevice;)Z", &[JValue::Object(&dev)])?.z()? {
                return Ok(Some(None));
            }
            let conn = env
                .call_method(&mgr, "openDevice", "(Landroid/hardware/usb/UsbDevice;)Landroid/hardware/usb/UsbDeviceConnection;", &[JValue::Object(&dev)])?
                .l()?;
            if conn.is_null() {
                return Ok(Some(Some(None)));
            }
            let fd = env.call_method(&conn, "getFileDescriptor", "()I", &[])?.i()?;
            let g = env.new_global_ref(conn)?;
            Ok(Some(Some(Some((g, fd)))))
        })?;
        match granted {
            None => return Err(io::Error::new(io::ErrorKind::NotFound, "dispositivo USB desconectado")),
            Some(Some(Some((g, fd)))) => {
                conns.lock().unwrap().insert(name.to_string(), (g, fd));
                return Ok(fd);
            }
            Some(Some(None)) => return Err(io::Error::other("o Android não abriu o dispositivo USB")),
            Some(None) if t0.elapsed() > Duration::from_secs(60) => {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "permissão USB não concedida"));
            }
            Some(None) => std::thread::sleep(Duration::from_millis(400)),
        }
    }
}

/// Esquece a conexão (ex.: após desconectar o dispositivo).
pub fn forget(name: &str) {
    if let Some(c) = CONNS.get() {
        if let Some((g, _)) = c.lock().unwrap().remove(name) {
            let _ = with_env(|env, _| {
                env.call_method(g.as_obj(), "close", "()V", &[])?;
                Ok(())
            });
        }
    }
}

fn find<'a>(env: &mut JNIEnv<'a>, mgr: &JObject, name: &str) -> JRes<Option<JObject<'a>>> {
    for d in devices(env, mgr)? {
        if safe_str(env, &d, "getDeviceName") == name {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// Margens das barras do sistema em pixels: [esq, topo, dir, base], via WindowInsets.
pub fn system_insets_px() -> Result<[i32; 4], String> {
    with_env(|env, act| {
        let win = env.call_method(act, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
        let decor = env.call_method(&win, "getDecorView", "()Landroid/view/View;", &[])?.l()?;
        let ins = env.call_method(&decor, "getRootWindowInsets", "()Landroid/view/WindowInsets;", &[])?.l()?;
        if ins.is_null() {
            return Ok(None);
        }
        let mut v = [0i32; 4];
        for (i, m) in ["getSystemWindowInsetLeft", "getSystemWindowInsetTop", "getSystemWindowInsetRight", "getSystemWindowInsetBottom"].iter().enumerate() {
            v[i] = env.call_method(&ins, m, "()I", &[])?.i()?;
        }
        Ok(Some(v))
    })
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "WindowInsets nulo".to_string())
}

/// Alturas da barra de status e de navegação pelos recursos do sistema (pixels).
pub fn bar_heights_px() -> Result<(i32, i32), String> {
    with_env(|env, _| {
        let res = env.call_static_method("android/content/res/Resources", "getSystem", "()Landroid/content/res/Resources;", &[])?.l()?;
        let mut out = [0i32; 2];
        for (i, name) in ["status_bar_height", "navigation_bar_height"].iter().enumerate() {
            let n = env.new_string(*name)?;
            let ty = env.new_string("dimen")?;
            let pkg = env.new_string("android")?;
            let id = env
                .call_method(&res, "getIdentifier", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I", &[JValue::Object(&n), JValue::Object(&ty), JValue::Object(&pkg)])?
                .i()?;
            if id != 0 {
                out[i] = env.call_method(&res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?.i()?;
            }
        }
        Ok((out[0], out[1]))
    })
    .map_err(|e| e.to_string())
}
