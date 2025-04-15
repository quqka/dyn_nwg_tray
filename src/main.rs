// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::{cell::RefCell, ffi::c_void, fs, os::windows::io::AsRawHandle, path::{Path, PathBuf}, process::Command, sync::mpsc::{channel, Receiver}, thread};
use windows::{core::PCWSTR, Win32::{Foundation::HANDLE, System::Threading::{GetExitCodeThread, GetThreadId, OpenThread, TerminateThread, THREAD_TERMINATE}, UI::WindowsAndMessaging::{AppendMenuW, CheckMenuItem, GetMenuItemCount, GetMenuStringW, RemoveMenu, HMENU, MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED, MF_STRING, MF_UNCHECKED}}};
extern crate native_windows_gui as nwg;
extern crate native_windows_derive as nwd;
use winreg::enums::*;
use winreg::RegKey;
use nwd::NwgUi;
use nwg::NativeUi;
use futures::{
    channel::mpsc::{channel as fchannel, Receiver as FReceiver},
    SinkExt, StreamExt,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use rhai::Engine;

fn simple_message(title: String, content: String){
    nwg::simple_message(&title, &content);
}

fn my_engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_fn("simple_message", simple_message);
    engine
}

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control]
    #[nwg_events( OnInit: [App::init] )]
    window: nwg::MessageWindow,

    #[nwg_resource(source_file: Some("./assets/app.ico"))]
    icon: nwg::Icon,

    #[nwg_control(icon: Some(&data.icon), tip: Some("Running"))]
    #[nwg_events(MousePressLeftUp: [App::show_menu], OnContextMenu: [App::show_menu])]
    tray: nwg::TrayNotification,

    #[nwg_control(parent: window, popup: true)]
    tray_menu: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "Open Script Folder")]
    #[nwg_events(OnMenuItemSelected: [App::open_folder])]
    open_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "New Script")]
    #[nwg_events(OnMenuItemSelected: [App::new_script])]
    new_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "Edit This Script")]
    #[nwg_events(OnMenuItemSelected: [App::edit_script])]
    edit_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "Reload This Script")]
    #[nwg_events(OnMenuItemSelected: [App::reload_script])]
    reload_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "Scripts")]
    scripts_item: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "Exit")]
    #[nwg_events(OnMenuItemSelected: [App::exit])]
    exit_item: nwg::MenuItem,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::update_scritps])]
    update_scritps_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::script_menuitem_event])]
    script_menuitem_event_notice: nwg::Notice,

    scripts_receiver: RefCell<Option<FReceiver<bool>>>,
    script_menuitem_event_receiver: RefCell<Option<Receiver<nwg::ControlHandle>>>,
    default_handler: RefCell<Option<nwg::EventHandler>>,

    this_script_name: RefCell<String>,
    script_thread_handle: RefCell<Option<thread::JoinHandle<()>>>,
}

impl App {
    fn init(&self) {
        self.setup_watcher();
        self.menuitem_event();
       
    }

    fn setup_watcher(&self) {
        let scripts_dir = Path::new("scripts");
        if !scripts_dir.exists() {
            fs::create_dir(scripts_dir).unwrap();
        }
        self.update_scritps_items();
        let (mut sender, receiver) = fchannel(100);
        let notice_sender = self.update_scritps_notice.sender();
        thread::spawn(move || {
            futures::executor::block_on(async {
                let (mut tx, mut rx) = fchannel(1);
                let mut watcher = RecommendedWatcher::new(
                    move |res| {
                        futures::executor::block_on(async {
                            tx.send(res).await.unwrap();
                        })
                    },
                    Config::default(),
                ).unwrap();

                watcher.watch(&scripts_dir, RecursiveMode::Recursive).unwrap();
            
                while let Some(res) = rx.next().await {
                    match res {
                        Ok(_) =>{
                            sender.send(true).await.unwrap();
                            notice_sender.notice();
                        },
                        Err(e) => println!("watch error: {:?}", e),
                    }
                }
            });
        });
        *self.scripts_receiver.borrow_mut() = Some(receiver);
    }

    fn menuitem_event(&self) {
        use nwg::Event as E;
        let window_handle = self.window.handle.clone();

        let (menuitem_sender, menuitem_receiver) = channel();
        let scrip_notice_sender = self.script_menuitem_event_notice.sender();
        
        let handle_events = {
            move |evt: nwg::Event, _evt_data: nwg::EventData, handle: nwg::ControlHandle| {
                match evt {
                    E::OnMenuItemSelected => 
                        {
                            menuitem_sender.send(handle).unwrap();
                            scrip_notice_sender.notice();
                            }
                    _ => {}
                }
            }
        };
        *self.script_menuitem_event_receiver.borrow_mut() = Some(menuitem_receiver);
        *self.default_handler.borrow_mut() = Some(nwg::full_bind_event_handler(&window_handle, handle_events));
    }

    fn run_script(&self, name: &String) {
        let script_file = PathBuf::from("scripts").join(name);
        if !script_file.exists() {
            nwg::simple_message("Error", "Script file not found!");
            return;
        }
        let join_handle = thread::spawn(
            move || {
                let engine = my_engine();
                let script = fs::read_to_string(script_file).unwrap();
                
                if let Err(e) = engine.run(&script) {
                    nwg::simple_message("Error", &e.to_string());
                }
            }
        );
        *self.script_thread_handle.borrow_mut() = Some(join_handle);
        let tray = &self.tray;
        tray.set_tip(&format!("Running {}", name));
    }


    fn update_scritps(&self) {
        let mut receiver_ref = self.scripts_receiver.borrow_mut();
        let receiver = receiver_ref.as_mut().unwrap();
        while let Ok(Some(data)) = receiver.try_next() {
            if data {
                self.update_scritps_items();
            }
        }
    }

    fn script_menuitem_event(&self) {
        let mut receiver_ref = self.script_menuitem_event_receiver.borrow_mut();
        let receiver = receiver_ref.as_mut().unwrap();
        while let Ok(data) = receiver.try_recv() {
            let Some((menu, id)) = data.hmenu_item() else {
                return;
            };
            if let Some(handle) = &*self.script_thread_handle.borrow() {
            // self.script_thread_handle.borrow().as_ref().map(|handle| {
                if self.is_thread_running(&handle) {
                    let p = nwg::MessageParams {
                        title: "Warning",
                        content: "Script is running, are you sure to stop it?",
                        buttons: nwg::MessageButtons::OkCancel,
                        icons: nwg::MessageIcons::Warning
                    };
                    if nwg::modal_message(&self.window, &p) == nwg::MessageChoice::Cancel {
                        return;
                    }
                    match self.thread_stop(&handle) {
                        Ok(_) => { println!("Thread stopped successfully"); }
                        Err(_) => return,
                    }
                }

            };
            let count = fs::read_dir("scripts").unwrap().count();

            if id >= 1100 && id < (1100 + count).try_into().unwrap()  {
                let hmenu = HMENU(menu as *mut c_void);
                let mut buffer: [u16; 256] = [0; 256];
                let len = unsafe { GetMenuStringW(hmenu, id - 1100, Some(&mut buffer),MF_BYPOSITION) };
                let script_name = String::from_utf16_lossy(&buffer[..len as usize]);
                *self.this_script_name.borrow_mut() = script_name.clone();
                self.set_menu_checked(hmenu, id);
                self.run_script(&script_name);
            }
        }
    }
    
    fn set_menu_checked(&self, hmenu: HMENU, id: u32) {
        let hchildren_count = unsafe { GetMenuItemCount(Some(hmenu)) };

        for i in (0..hchildren_count).rev() {
            unsafe { CheckMenuItem(
                hmenu,
                (1100 + i).try_into().unwrap(),
                (MF_UNCHECKED | MF_BYCOMMAND).0,
            ) };                                    
        }

        unsafe { CheckMenuItem(
            hmenu,
            id,
            (MF_CHECKED | MF_BYCOMMAND).0,
        )};
    }

    fn update_scritps_items(&self) {
        let paths = fs::read_dir("scripts").unwrap();
        self.remove_sub_menu(&self.scripts_item);
        let mut script_menuitem_id = 1100;
        for path in paths {
            if let Ok(path) = path {
                if path.path().is_dir() { continue; }
                if path.path().extension().unwrap() != "rhai" { continue; }
                self.add_sub_menu(&self.scripts_item, &path.file_name().to_string_lossy().to_string(), script_menuitem_id);
                script_menuitem_id += 1;
            } 
        }
    }


    pub fn add_sub_menu(&self, menu: &nwg::Menu, text: &str, id: i32) {
        if menu.handle.blank() {
            return;
        }
        let Some((_, menu)) = menu.handle.hmenu() else {
            return;
        };
        let hmenu = HMENU(menu as *mut c_void);
        let wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let this_script_name_ref = self.this_script_name.borrow();
        let script_name = this_script_name_ref.as_str();
        unsafe { 
            let _ = AppendMenuW(hmenu, MF_STRING, id.try_into().unwrap(), PCWSTR(wide_text.as_ptr()));
            if text == script_name {
                CheckMenuItem(hmenu, (id).try_into().unwrap(),(MF_CHECKED | MF_BYCOMMAND).0);
            }
        }
    }
    
    pub fn remove_sub_menu(&self, menu: &nwg::Menu) {
        if menu.handle.blank() {
            return;
        }
        let Some((_, menu)) = menu.handle.hmenu() else {
            return;
        };
        let hmenu = HMENU(menu as *mut c_void);
        let hchildren_count = unsafe { GetMenuItemCount(Some(hmenu)) };
        for i in (0..hchildren_count).rev() {
            unsafe { let _ = RemoveMenu(hmenu, i as u32, MF_BYPOSITION); };
        }
    }

    fn show_menu(&self) {
        let (x, y) = nwg::GlobalCursor::position();
        self.tray_menu.popup(x, y);
    }
    fn open_folder(&self) {
        let path = PathBuf::from("scripts");
        if path.exists() {
            Command::new("explorer.exe").arg(path).spawn().unwrap();
        } else {
            nwg::simple_message("Error", "Scripts folder not found!");
        }
    }

    fn new_script(&self) {
        match self.get_txt_default_program() {
            Ok(program) => { 
                Command::new(program).arg("").spawn().expect("Failed to open text editor");
            },
            Err(e) => eprintln!("Error: {}", e),
        }
    }

    fn reload_script(&self) {
        let this_script_name_ref = self.this_script_name.borrow();
        let script_name = this_script_name_ref.as_str();
        if script_name == "" {
            return;
        }
        self.run_script(&script_name.to_string());
    }
    
    fn edit_script(&self) {
        match self.get_txt_default_program() {
            Ok(program) => { 
                let this_script_name_ref = self.this_script_name.borrow();
                let script_name = this_script_name_ref.as_str();
                if script_name == "" {
                    nwg::simple_message("Error", "No script selected!");
                    return;
                }
                let path = PathBuf::from("scripts").join(script_name);
                
                Command::new(program).arg(path).spawn().unwrap();
            },
            Err(e) => eprintln!("Error: {}", e),
        }
    }
    
    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }
    fn get_txt_default_program(&self) -> Result<String, Box<dyn std::error::Error>> {
        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let txt_ext_key = hkcr.open_subkey(".txt")?;
        let file_type: String = txt_ext_key.get_value("")?;
    
        let command_path = format!("{}\\shell\\open\\command", file_type);
        let command_key = hkcr.open_subkey(command_path)?;
        let command: String = command_key.get_value("")?;
    
        let path = command
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches('"');
        // let system_root = path.split('\\').next().unwrap_or_default();
        let path = path.replace("%SystemRoot%", env!("SystemRoot"));
        Ok(path.to_string())
    }

    fn is_thread_running(&self, handle: &thread::JoinHandle<()>) -> bool {
        let mut exit_code = 0;
        let raw_handle = handle.as_raw_handle();
        unsafe {
            match GetExitCodeThread(HANDLE(raw_handle as *mut c_void), &mut exit_code) {
                Ok(_) => exit_code == 259, // STILL_ACTIVE
                Err(_) => false,
            }
        }
    }
    fn thread_stop(&self, handle: &thread::JoinHandle<()>) -> Result<(), anyhow::Error> {
        let raw_handle = handle.as_raw_handle() as isize;

        let thread_handle = unsafe {
            OpenThread(THREAD_TERMINATE, false, GetThreadId(HANDLE(raw_handle as *mut c_void)))?
        };
        
        unsafe {
            TerminateThread(thread_handle, 0).ok();
        }
    
        // unsafe {
        //     windows::Win32::Foundation::CloseHandle(thread_handle).ok();
        // }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    nwg::init().expect("Failed to init Native Windows GUI");
    let _ui = App::build_ui(Default::default()).expect("Failed to build UI");
    nwg::dispatch_thread_events();
    Ok(())
}