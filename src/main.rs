use std::{
    fs::{self},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use ::time::format_description;
use path_clean::PathClean;
use reqwest::header::{HeaderName, HeaderValue};
use time::UtcOffset;

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
use winapi::um::securitybaseapi::GetTokenInformation;
use winapi::um::winnt::{TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};

fn get_apath(path: &PathBuf) -> PathBuf {
    let apath;
    if path.is_absolute() {
        apath = path.clean();
    } else {
        apath = std::env::current_dir().unwrap().join(path).clean();
    }
    apath
}

fn get_qq_path_by_reg() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
    let qq_setting;
    if let Ok(val) = hkcu.open_subkey(r#"Software\Microsoft\Windows\CurrentVersion\Uninstall\QQ"#) {
        qq_setting = val;
    } else {
        qq_setting = hkcu.open_subkey(r#"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\QQ"#)?;
    }
    let qq_path: String = qq_setting.get_value("UninstallString")?;
    let q = PathBuf::from_str(&qq_path)?
        .parent()
        .ok_or("can't find qq path")?
        .to_owned();
    Ok(q)
}

fn get_qq_path_by_current_exe_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let current_exe_path = std::env::current_exe()?;
    let current_path = current_exe_path.parent().ok_or("can't find current path")?;
    let qq_path = current_path.join("QQ.exe");
    if qq_path.is_file() {
        return Ok(current_path.to_path_buf());
    }
    Err("can't find qq.exe on current path".into())
}

fn get_qq_path_by_cfg() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let current_exe_path = std::env::current_exe()?;
    let current_path = current_exe_path.parent().ok_or("can't find current path")?;
    let cfg_file = current_path.join("llob_install.json");
    let json_str = fs::read_to_string(cfg_file)?;
    let json: serde_json::Value = serde_json::from_str(&json_str)?;
    let qq_path_str = json["qq_exe_path"]
        .as_str()
        .ok_or("failed to get qq_exe_path")?;
    let qq_exe_path = PathBuf::from(qq_path_str);
    let qq_exe_path_t = get_apath(&qq_exe_path);
    if qq_exe_path_t.is_file() {
        return Ok(qq_exe_path_t
            .parent()
            .ok_or("can't find qq path")?
            .to_path_buf());
    }
    Err("can't find qq.exe llob_install.json".into())
}

fn get_qq_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    // 先看配置文件
    if let Ok(qq_path) = get_qq_path_by_cfg() {
        log::info!("从配置文件获取到QQ.exe");
        return Ok(qq_path);
    }
    // 再看当前目录
    if let Ok(qq_path) = get_qq_path_by_current_exe_path() {
        log::info!("从当前位置获取到QQ.exe");
        return Ok(qq_path);
    }
    // 再看注册表
    if let Ok(qq_path) = get_qq_path_by_reg() {
        log::info!("从注册表获取到QQ.exe");
        return Ok(qq_path);
    }
    Err("can't find qq path".into())
}

fn is_qq_run(qq_path:&PathBuf) -> Result<bool, Box<dyn std::error::Error>>  {
    let system = sysinfo::System::new_all();
    let process_name = "QQ.exe";
    if let Some(process) = system.processes_by_name(process_name).next() {
        let process_exe_path = process.exe().ok_or("can't get process exe path")?;
        if let Some(process_path) = process_exe_path.parent() {
            if process_path == qq_path {
                return Ok(true)
            }
        }
    } 
    Ok(false)
}

fn http_post(rt_ptr: Arc<tokio::runtime::Runtime>, url: &str, user_agent: Option<&str>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bin = rt_ptr.block_on(async {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .build()
            .unwrap();
        let mut req = client
            .get(url)
            .body(reqwest::Body::from(vec![]))
            .build()
            .unwrap();
        if let Some(ua) = user_agent {
            req.headers_mut().append(
                HeaderName::from_str("User-Agent").unwrap(),
                HeaderValue::from_str(ua).unwrap(),
            );
        }
        let ret = client.execute(req).await;
        if ret.is_err() {
            log::error!("Failed to download file{:?}", ret.err().unwrap());
            return Err("Failed to download file".into());
        }
        let ret = ret.unwrap();
        let bin = ret.bytes().await;
        if bin.is_err() {
            log::error!("Failed to download file{:?}", bin.err().unwrap());
            return Err("Failed to download file".into());
        }
        let bin = bin.unwrap();
        Ok(bin.to_vec())
    });
    bin
}

fn is_admin() -> Result<bool, Box<dyn std::error::Error>> {
    let mut token: winapi::um::winnt::HANDLE = null_mut();
    let process = unsafe { GetCurrentProcess() };

    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } != 0 {
        let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
        let mut ret_length = 0;

        let success = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as winapi::shared::minwindef::LPVOID,
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_length,
            )
        };

        unsafe { CloseHandle(token) };

        if success != 0 && elevation.TokenIsElevated != 0 {
            Ok(true)
        } else {
            Ok(false)
        }
    } else {
        Ok(false)
    }
}

fn init_log() {
    // 初始化日志
    let format = "[year]-[month]-[day] [hour]:[minute]:[second]";

    // 获得utc偏移
    let utc_offset;
    if let Ok(v) = UtcOffset::current_local_offset() {
        utc_offset = v;
    } else {
        // 中国是东八区，所以这里写8 hour
        utc_offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    }

    tracing_subscriber::fmt()
        .with_timer(tracing_subscriber::fmt::time::OffsetTime::new(
            utc_offset,
            format_description::parse(format).unwrap(),
        ))
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .init();
}

fn app_exit() -> ! {
    loop {
        let time_struct = core::time::Duration::from_millis(500);
        std::thread::sleep(time_struct);
    }
}

fn is_x86_64(exe_data: &[u8]) -> Result<bool, Box<dyn std::error::Error>> {
    use goblin::Object;
    match Object::parse(exe_data)? {
        Object::PE(pe) => Ok(pe.is_64),
        _ => Err("File is not a Windows PE file.".into()),
    }
}

fn iswin32(qq_exe_path: &PathBuf) -> Result<bool, Box<dyn std::error::Error>> {
    let content = std::fs::read(qq_exe_path)?;
    if is_x86_64(&content)? {
        return Ok(false);
    }
    Ok(true)
}

pub async fn github_proxy() -> Option<String> {
    let urls_to_test = [
        "https://kkgithub.com",
        "https://dgithub.xyz",
        "https://gh.jiasu.in/https://github.com",
        "https://github.com",
    ];
    let (tx, mut rx) = tokio::sync::mpsc::channel(urls_to_test.len() + 1);
    for url in urls_to_test {
        let tx = tx.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .no_proxy()
                .build()
                .unwrap();
            let uri = reqwest::Url::from_str(&(url.to_owned() + "/LiteLoaderQQNT/QQNTFileVerifyPatch/releases/download/DllHijack_1.0.8/dbghelp_x64.dll")).unwrap();
            let req = client.get(uri).build().unwrap();
            if let Ok(ret) = client.execute(req).await {
                if ret.status() == reqwest::StatusCode::OK {
                    if let Ok(bin) = ret.bytes().await {
                        if bin.starts_with(&[b'M', b'Z']) {
                            let _err = tx.send(url).await;
                        }
                    }
                }
            };
        });
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _err = tx.send("timeout").await;
    });
    let ret = rx.recv().await;
    if let Some(r) = ret {
        if r != "timeout" {
            return Some(r.to_owned());
        }
    }
    None
}

fn fix_index_js(index_js_path:&PathBuf,userdir:&PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut to_write = r#"const fs = require("fs");
const path = require("path");
const package_path = path.join(process.resourcesPath, "app/package.json");
const package = require(package_path);
package.main = "./application/app_launcher/index.js";
fs.writeFileSync(package_path, JSON.stringify(package, null, 4), "utf-8");
"#.to_owned();
    to_write.push_str(&("require(String.raw`".to_owned()
    + &userdir
        .join("LiteLoaderQQNT-main")
        .to_string_lossy()
        .to_string()
    + "`);\r\n"));
    to_write.push_str("require('../major.node').load('internal_index', module);\r\n");

    to_write.push_str("setTimeout(() => {\n");
    to_write.push_str("    package.main = \"./app_launcher/index.js\";\n");
    to_write.push_str("    fs.writeFileSync(package_path, JSON.stringify(package, null, 4), \"utf-8\");\n");
    to_write.push_str("}, 0);\n");

    fs::write(
        index_js_path,
        to_write,
    )?;
    Ok(())
}

fn extrat(from: &PathBuf, to: &PathBuf, flag: bool) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(from)?;

    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => {
                // write by chatgpt4
                let deal_path = path;
                let components: Vec<_> = deal_path.components().collect();
                if flag {
                    // println!("components:{components:?}");
                    if components.len() > 1 {
                        // 从第二个组件开始收集，直到倒数第二个（不包括最后一个组件）
                        let new_path = components[1..components.len()]
                            .iter()
                            .map(|c| c.as_os_str())
                            .collect::<PathBuf>();
                        to.join(new_path)
                    } else {
                        continue;
                        //return Err("Path is too short to remove the last component".into());
                    }
                } else {
                    let new_path = components[0..components.len()]
                        .iter()
                        .map(|c| c.as_os_str())
                        .collect::<PathBuf>();
                    to.join(new_path)
                }
            }
            None => continue,
        };

        {
            let comment = file.comment();
            if !comment.is_empty() {
                log::error!("File {i} comment: {comment}");
            }
        }

        if (*file.name()).ends_with('/') {
            // log::info!("File {} extracted to \"{}\"", i, outpath.display());
            std::fs::create_dir_all(&outpath)?;
        } else {
            // log::info!(
            //     "File {} extracted to \"{}\" ({} bytes)",
            //     i,
            //     outpath.display(),
            //     file.size()
            // );
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(p)?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }

        // Get and Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Some(mode) = file.unix_mode() {
                std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
}

fn get_qq_version(qqpath:&PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    let config_json_path = qqpath.join("versions").join("config.json");
    let config_str = fs::read_to_string(config_json_path)?;
    let config_json:serde_json::Value = serde_json::from_str(&config_str)?;
    let cur_version = config_json["curVersion"].as_str().ok_or("解析config.json失败")?;
    return Ok(cur_version.to_owned());
}

fn main() {
    if let Err(e) = mymain() {
        log::error!("{e:?}");
        app_exit();
    }
    app_exit();
}

fn fix_package_json(package_json_path:&PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let json_str = fs::read_to_string(package_json_path)?;
    let mut json:serde_json::Value = serde_json::from_str(&json_str)?;
    let json_main = json.get_mut("main").ok_or("没有在package.json中找到main字段")?;
    *json_main = serde_json::json!("./app_launcher/index.js");
    fs::write(
        package_json_path,
        serde_json::to_string_pretty(&json)?,
    )?;
    Ok(())
}

fn mymain() -> Result<(), Box<dyn std::error::Error>> {
    let rt_ptr: Arc<tokio::runtime::Runtime> = Arc::new(tokio::runtime::Runtime::new().unwrap());

    init_log();

    log::info!("欢迎使用LLOB安装器0.0.9 by super1207");

    if let Ok(_) = std::env::var("LITELOADERQQNT_PROFILE") {
        log::error!("检测到您的环境变量中存在LITELOADERQQNT_PROFILE，你可能已经手动安装过LiteLoaderQQNT，程序终止！");
        app_exit();
    }

    let has_admin = is_admin().unwrap();
    if has_admin {
        log::info!("拥有管理员权限");
    } else {
        log::error!("没有管理员权限");
        app_exit();
    }

    log::info!("正在查询QQ安装位置...");
    let qq_path;
    if let Ok(qq_path_t) = get_qq_path() {
        qq_path = qq_path_t;
        log::info!("QQ安装位置: {:?}", qq_path);
    } else {
        log::error!("未找到QQ安装位置,请去安装QQ!：https://im.qq.com/pcqq/index.shtml");
        app_exit();
    }

    let qq_version = match get_qq_version(&qq_path) {
        Ok(ver) => ver,
        Err(err) => {
            log::error!("获取QQ版本号失败,注意当前安装器支持的最低NTQQ版本是9.9.15-28060:{err:?}\r\n如果您使用之前的NTQQ版本，请使用旧版安装器!");
            app_exit();
        },
    };

    let qq_inner_path = qq_path.join("versions").join(qq_version).join("resources").join("app");
    let package_json_path = qq_inner_path.join("package.json");
    let index_js_path = qq_inner_path.join("app_launcher").join("index.js");


    match is_qq_run(&qq_path) {
        Ok(is_run) => {
            if !is_run {
                // log::info!("QQ未运行");
            } else {
                log::error!("QQ正在运行，安装LLONEBOT需要确保QQ处于未运行状态，请先结束QQ");
                app_exit();
            }
        }
        Err(err) => {
            log::error!("无法检查QQ是否正在运行:{err:?}");
            app_exit();
        }
    }
    let is_win32 = iswin32(&qq_path.join("QQ.exe"))?;

    log::info!("正在获取github下载代理...");
    let git_proxy = rt_ptr.block_on(async {
        if let Some(proxy_t) = github_proxy().await {
            if proxy_t == "https://github.com" {
                log::info!("无需使用代理即可连接github");
            } else {
                log::info!("使用代理: {:?}", proxy_t);
            }
            return proxy_t;
        } else {
            log::error!("无法获取github代理");
            app_exit();
        }
    });

    log::info!("正在获取最新QQNTFileVerifyPatch版本号...");
    let url = "http://47.113.226.225:12341/";
    let bin = match http_post(rt_ptr.clone(), url, Some("Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/89.0.4389.72 Safari/537.36")) {
        Ok(bin) => bin,
        Err(_) => {
            log::warn!("无法访问GitHub，尝试使用备用URL");
            let backup_url = "http://47.113.226.225:12341/";
            match http_post(rt_ptr.clone(), backup_url, Some("Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/89.0.4389.72 Safari/537.36")) {
                Ok(bin) => bin,
                Err(_) => {
                    log::error!("无法获取最新QQNTFileVerifyPatch版本号");
                    app_exit();
                }
            }
        }
    };
    let version_json: serde_json::Value = serde_json::from_slice(&bin)?;
    let tag_name = version_json["tag_name"]
        .as_str()
        .ok_or("Failed to get tag_name")?;
    log::info!("最新QQNTFileVerifyPatch版本号:{tag_name}");

    log::info!("正在下载修补文件...");
    let patch_url;
    if is_win32 {
        patch_url = format!("{git_proxy}/LiteLoaderQQNT/QQNTFileVerifyPatch/releases/download/{tag_name}/dbghelp_x86.dll");
    } else {
        patch_url = format!("{git_proxy}/LiteLoaderQQNT/QQNTFileVerifyPatch/releases/download/{tag_name}/dbghelp_x64.dll");
    }
    let bin = match http_post(rt_ptr.clone(), &patch_url, None) {
        Ok(bin) => bin,
        Err(_) => {
            log::error!("修补文件下载失败");
            app_exit();
        }
    };
    log::info!("修补文件下载完成");

    log::info!("正在修补...");
    fs::write(qq_path.join("dbghelp.dll"), bin)?;
    log::info!("修补完成");

    log::info!("正在下载LiteLoader项目...");
    let patch_url = format!("{git_proxy}/LiteLoaderQQNT/LiteLoaderQQNT/archive/master.zip");
    let bin = match http_post(rt_ptr.clone(), &patch_url, None) {
        Ok(bin) => bin,
        Err(_) => {
            log::error!("LiteLoader项目下载失败");
            app_exit();
        }
    };
    log::info!("下载完成");

    log::info!("正在解压...");
    let userdir = PathBuf::from_str(&std::env::var("USERPROFILE")?)?;
    let zip_path = userdir.join("LiteLoaderQQNT-main.zip");
    fs::write(&zip_path, bin)?;
    extrat(
        &zip_path,
        &zip_path
            .parent()
            .ok_or("can't get parent")?
            .join("LiteLoaderQQNT-main"),
        true,
    )?;
    log::info!("解压完成");
    fix_index_js(&index_js_path,&userdir)?;
    fix_package_json(&package_json_path)?;
    log::info!("LiteLoaderQQNT安装完成");

    log::info!("正在获取最新LLOB版本号...");
    let url = "http://47.113.226.225:12341/";
    let bin = match http_post(rt_ptr.clone(), url, Some("Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/89.0.4389.72 Safari/537.36")) {
        Ok(bin) => bin,
        Err(_) => {
            log::warn!("无法访问GitHub，尝试使用备用URL");
            let backup_url = "http://47.113.226.225:12341/";
            match http_post(rt_ptr.clone(), backup_url, Some("Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/89.0.4389.72 Safari/537.36")) {
                Ok(bin) => bin,
                Err(_) => {
                    log::error!("无法获取最新LLOB版本号");
                    app_exit();
                }
            }
        }
    };
    let version_json: serde_json::Value = serde_json::from_slice(&bin)?;
    let tag_name = version_json["tag_name"]
        .as_str()
        .ok_or("Failed to get tag_name")?;
    log::info!("最新LLOB版本号:{tag_name}");

    log::info!("正在下载LLOB项目...");
    let patch_url = format!("{git_proxy}/LLOneBot/LLOneBot/releases/download/{tag_name}/LLOneBot.zip");
    let bin = match http_post(rt_ptr.clone(), &patch_url, None) {
        Ok(bin) => bin,
        Err(_) => {
            log::error!("LLOB项目下载失败");
            app_exit();
        }
    };
    log::info!("下载完成");

    log::info!("正在安装LLOnebOT...");
    let zip_path = userdir
        .join("LiteLoaderQQNT-main")
        .join("plugins")
        .join(format!("LLOneBot{tag_name}.zip"));
    std::fs::create_dir_all(zip_path.parent().ok_or("can't get parent")?)?;
    // 有时候没这个目录会报错
    std::fs::create_dir_all(userdir.join("LiteLoaderQQNT-main").join("data"))?;
    fs::write(&zip_path, bin)?;
    extrat(
        &zip_path,
        &zip_path
            .parent()
            .ok_or("can't get parent")?
            .join("LLOneBot"),
        false,
    )?;
    log::info!("安装完成");

    log::info!("安装成功！！！！！！！！！享受快乐时光吧");

    Ok(())
}
