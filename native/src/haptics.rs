//! Optional system haptics; never override the user's vibration preferences.
use crate::util::{self,Result};
use std::time::Duration;
fn effect(action:&str)->Result<(&'static str,&'static str)>{match action{"tick"=>Ok(("9","5")),"select"=>Ok(("1","8")),"toggle"=>Ok(("1","12")),"success"=>Ok(("16","16")),"failure"=>Ok(("17","24")),"warning"=>Ok(("17","18")),_=>Err("未知触感动作".into())}}
pub fn run(action:&str)->Result<()> {
 if unsafe{crate::ffi::geteuid()}!=0{return Err("触感系统通道需要 root".into());}
 if action!="probe"{effect(action)?;}
 let query=|args:&[&str]|util::query_command("/system/bin/cmd",args,Duration::from_secs(1));
 let help=query(&["vibrator_manager","help"]).unwrap_or_default();
 let feedback=help.contains("feedback")&&help.contains("<constant>");
 if !feedback&&!(help.contains("synced")&&help.contains("oneshot")){return Err("系统没有兼容的触感命令".into());}
 if action=="probe"{println!("haptic=system");return Ok(());}
 let (constant,duration)=effect(action)?;
 if feedback{query(&["vibrator_manager","feedback",constant])?;}else{
  let enabled=util::query_command("/system/bin/settings",&["--user","current","get","system","haptic_feedback_enabled"],Duration::from_secs(1))?;
  if enabled.trim()!="1"{return Ok(());}
  query(&["vibrator_manager","synced","oneshot",duration])?;
 }Ok(())
}
#[cfg(test)]mod tests{use super::*;#[test]fn only_fixed_semantic_effects_are_accepted(){for a in ["tick","select","toggle","success","failure","warning"]{assert!(effect(a).is_ok());}for a in ["","-f","select;reboot","10000"]{assert!(effect(a).is_err());}}}
