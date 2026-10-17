//! Private layout and migration. Never move an older daemon's active lock.
use crate::{process_identity, util::{self, Result}};
use std::{fs, path::{Path, PathBuf}, io::{Read, Seek, SeekFrom}};
use std::os::unix::fs::PermissionsExt;
pub const FILES: &[&str] = &[
 "config.json","config.default.json","config.profile","app_modes.txt","automatic.json","options.txt",
 "mode.txt","scene-request.txt","daemon.identity","daemon.pid","daemon.lock","supervisor.lock","control.lock",
 "config.install.lock","webui-start.lock","webui-start.boot","stop.requested","status","startup","boot-guard","last_error",
 "webui.session","webui.origins","webui-origins.lock","novasched.log","bootstrap.log","boot-entry.log","install-last.log","stock_snapshot.tsv","scene-provider",
];
pub fn target(root:&Path,name:&str)->PathBuf {
 let (folder,name)=match name {
  "config.json"|"config.default.json"|"config.profile"|"app_modes.txt"|"automatic.json"|"options.txt"=>("config",name),
  "novasched.log"|"bootstrap.log"|"boot-entry.log"=>("logs",name),
  "install-last.log"=>("logs","install.log"),
  "stock_snapshot.tsv"|"config.previous.json"=>("restore",name),
  "scene-provider"=>("restore","scene"), _=>("run",name),
 };root.join(folder).join(name)
}
pub fn path(root:&Path,name:&str)->PathBuf {
 let new=target(root,name);let old=root.join(name);
 if old.exists()&&!new.exists(){old}else{new}
}
fn private_dir(path:&Path)->Result<()> {
 match fs::symlink_metadata(path) {
  Ok(m) if m.is_dir()&&!m.file_type().is_symlink()=>{},
  Ok(_)=>return Err(format!("数据目录不安全: {}",path.display())),
  Err(e) if e.kind()==std::io::ErrorKind::NotFound=>util::create_dir(path)?,
  Err(e)=>return Err(e.to_string()),
 }
 fs::set_permissions(path,fs::Permissions::from_mode(0o700)).map_err(|e|e.to_string())
}
pub fn prepare(root:&Path)->Result<bool> {
 private_dir(root)?;
 for folder in ["config","run","logs","restore"]{private_dir(&root.join(folder))?;}
 let _migration=util::acquire_flock(&root.join("run/storage.lock"))?;
 let mut locks=Vec::new();
 for name in ["control.lock","daemon.lock","supervisor.lock","config.install.lock"] {
  let old=root.join(name);
  if old.exists() {
   if process_identity::lock_held(&old)?{return Ok(false);}
   match util::acquire_flock(&old){Ok(lock)=>locks.push(lock),Err(_)=>return Ok(false)}
  }
 }
 for name in FILES {
  let old=root.join(name);let new=target(root,name);
  if let Ok(meta)=fs::symlink_metadata(&old) {
   if meta.file_type().is_symlink(){return Err(format!("拒绝迁移链接: {}",old.display()));}
   if new.exists()&&*name!="install-last.log"{return Err(format!("发现重复数据，已保留两份: {} / {}",old.display(),new.display()));}
  }
 }
 for name in FILES {
  let old=root.join(name);let new=target(root,name);
  if old.exists(){
   if *name=="install-last.log"&&new.exists(){fs::remove_file(old)}else{fs::rename(old,new)}.map_err(|e|format!("迁移 {name} 失败: {e}"))?;
  }
 }
 for name in ["stock_snapshot.previous.tsv","novasched.log.1","bootstrap.log.1"]{util::remove_if_exists(&root.join(name))?;}
 let mut history=Vec::new();
 for entry in fs::read_dir(root).map_err(|e|e.to_string())? {
  let entry=entry.map_err(|e|e.to_string())?;let name=entry.file_name().to_string_lossy().into_owned();
  if ((name.starts_with("config.")&&name.contains(".before-")&&name.ends_with(".json"))||name=="config.json.bak")&&entry.file_type().is_ok_and(|t|t.is_file()){history.push(entry.path());}
 }
 history.sort_by_key(|p|fs::metadata(p).and_then(|m|m.modified()).ok());
 let previous=target(root,"config.previous.json");
 if !previous.exists(){if let Some(latest)=history.pop(){fs::rename(latest,previous).map_err(|e|e.to_string())?;}}
 for file in history{util::remove_if_exists(&file)?;}
 let request=target(root,"scene-request.txt");
 if fs::read_to_string(&request).is_ok_and(|s|s.lines().any(|l|l=="action=pedestal")) {
  if fs::read_to_string(target(root,"mode.txt")).is_ok_and(|s|s.trim()=="fast"){util::atomic_write(&target(root,"mode.txt"),b"powersave\n",0o600)?;}
  util::remove_if_exists(&request)?;
 }
 Ok(true)
}
pub fn ensure_ready(root:&Path)->Result<()> {
 if prepare(root)?{Ok(())}else{Err("旧版服务仍在使用原目录；安装已保留数据，请重启手机完成迁移".into())}
}
pub fn cleanup_legacy(module:&Path,root:&Path)->Result<()> {
 if !fs::read_to_string(module.join("module.prop")).is_ok_and(|s|s.lines().any(|l|l=="id=NovaSched_Zen_Edition")){return Err("模块身份不匹配，取消清理".into());}
 for entry in fs::read_dir(module).map_err(|e|e.to_string())? {
  let entry=entry.map_err(|e|e.to_string())?;let name=entry.file_name().to_string_lossy().into_owned();let kind=entry.file_type().map_err(|e|e.to_string())?;
  let old=name=="boot-entry.log"||name=="install-check.log"||(name.starts_with("install-")&&(name.ends_with(".log")||name.ends_with(".exit")));
  if old&&kind.is_file(){
   if name.ends_with(".log"){
    let mut file=fs::File::open(entry.path()).map_err(|e|e.to_string())?;
    let size=file.metadata().map_err(|e|e.to_string())?.len();file.seek(SeekFrom::Start(size.saturating_sub(524288))).map_err(|e|e.to_string())?;
    let mut bytes=Vec::new();file.take(524288).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
    crate::logging::append_bounded(&target(root,if name=="boot-entry.log"{"boot-entry.log"}else{"install-last.log"}),&String::from_utf8_lossy(&bytes)).map_err(|e|e.to_string())?;
   }util::remove_if_exists(&entry.path())?;
  }
  if !module.join(".webui-update.lock").exists()&&kind.is_dir()&&(name.starts_with(".webroot-before-update-")||name.starts_with(".webroot-new-"))&&module.join("webroot/index.html").is_file(){fs::remove_dir_all(entry.path()).map_err(|e|e.to_string())?;}
 }
 let mut mirrors=vec![PathBuf::from("/sdcard/Android/NovaSched")];
 for base in ["/storage/emulated","/data/media"]{if let Ok(entries)=fs::read_dir(base){for e in entries.flatten(){if e.file_type().is_ok_and(|t|t.is_dir())&&e.file_name().to_string_lossy().parse::<u32>().is_ok(){mirrors.push(e.path().join("Android/NovaSched"));}}}}
 for dir in mirrors {
  if !fs::symlink_metadata(&dir).is_ok_and(|m|m.is_dir()&&!m.file_type().is_symlink()){continue;}
  for name in ["log.txt","log.log.1"]{let p=dir.join(name);if fs::symlink_metadata(&p).is_ok_and(|m|m.is_file()){let _=fs::remove_file(p);}}
  let _=fs::remove_dir(dir);
 }
 cleanup_deferred_scripts(Path::new("/data/adb/service.d"))
}
pub(crate) fn cleanup_deferred_scripts(dir:&Path)->Result<()> {
 let entries=match fs::read_dir(dir){Ok(e)=>e,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()),Err(e)=>return Err(e.to_string())};
 for e in entries {
  let e=e.map_err(|e|e.to_string())?;let n=e.file_name().to_string_lossy().into_owned();
  if !n.starts_with("novasched-uninstall-cleanup-")||!n.ends_with(".sh")||!e.file_type().is_ok_and(|t|t.is_file()){continue;}
  let mut header=String::new();fs::File::open(e.path()).map_err(|e|e.to_string())?.take(4096).read_to_string(&mut header).map_err(|e|e.to_string())?;
  if header.lines().any(|l|l=="# NovaSched_Zen_Edition_Uninstall_Cleanup")&&header.lines().any(|l|l=="NOVA_RUNTIME=/data/adb/novasched"){util::remove_if_exists(&e.path())?;}
 }Ok(())
}
#[cfg(test)]mod tests {
 use super::*;
 fn fixture(name:&str)->PathBuf{let p=std::env::temp_dir().join(format!("nova-layout-{}-{name}",std::process::id()));let _=fs::remove_dir_all(&p);fs::create_dir_all(&p).unwrap();p}
 #[test]fn migration_preserves_rules_preferences_and_first_scene_originals(){let r=fixture("preserve");for n in ["config.json","app_modes.txt","automatic.json","stock_snapshot.tsv"]{fs::write(r.join(n),n).unwrap();}fs::create_dir(r.join("scene-provider")).unwrap();fs::write(r.join("scene-provider/powercfg.sh.backup"),"original").unwrap();assert!(prepare(&r).unwrap());assert!(prepare(&r).unwrap());assert_eq!(fs::read_to_string(r.join("config/app_modes.txt")).unwrap(),"app_modes.txt");assert_eq!(fs::read_to_string(r.join("config/automatic.json")).unwrap(),"automatic.json");assert_eq!(fs::read_to_string(r.join("restore/stock_snapshot.tsv")).unwrap(),"stock_snapshot.tsv");assert_eq!(fs::read_to_string(r.join("restore/scene/powercfg.sh.backup")).unwrap(),"original");assert_eq!(fs::read_dir(&r).unwrap().count(),4);fs::remove_dir_all(r).unwrap();}
 #[test]fn active_legacy_lock_defers_migration_without_replacing_inode(){let r=fixture("live");fs::write(r.join("config.json"),"custom").unwrap();let lock=util::acquire_flock(&r.join("daemon.lock")).unwrap();assert!(!prepare(&r).unwrap());assert!(r.join("config.json").exists());assert!(!r.join("run/daemon.lock").exists());drop(lock);assert!(prepare(&r).unwrap());fs::remove_dir_all(r).unwrap();}
 #[test]fn conflicting_data_is_preserved_and_reported(){let r=fixture("conflict");prepare(&r).unwrap();fs::write(r.join("config.json"),"old").unwrap();fs::write(r.join("config/config.json"),"new").unwrap();assert!(prepare(&r).is_err());assert_eq!(fs::read_to_string(r.join("config.json")).unwrap(),"old");fs::remove_dir_all(r).unwrap();}
 #[test]fn one_history_and_removed_pedestal_does_not_pin_fast(){let r=fixture("history");for n in ["config.SM8650.before-SM8650.1.1.json","config.SM8650.before-SM8650.2.1.json"]{fs::write(r.join(n),"tuning").unwrap();}fs::write(r.join("scene-request.txt"),"action=pedestal\n").unwrap();fs::write(r.join("mode.txt"),"fast\n").unwrap();prepare(&r).unwrap();assert!(r.join("restore/config.previous.json").exists());assert_eq!(fs::read_to_string(r.join("run/mode.txt")).unwrap(),"powersave\n");assert_eq!(fs::read_dir(&r).unwrap().count(),4);fs::remove_dir_all(r).unwrap();}
 #[test]fn owned_cleanup_scripts_require_name_and_header(){let r=fixture("scripts");let h="# NovaSched_Zen_Edition_Uninstall_Cleanup\nNOVA_RUNTIME=/data/adb/novasched\n";fs::write(r.join("novasched-uninstall-cleanup-1-1.sh"),h).unwrap();fs::write(r.join("foreign.sh"),h).unwrap();fs::write(r.join("novasched-uninstall-cleanup-user.sh"),"user").unwrap();cleanup_deferred_scripts(&r).unwrap();assert_eq!(fs::read_dir(&r).unwrap().count(),2);fs::remove_dir_all(r).unwrap();}
}
