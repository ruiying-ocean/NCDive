use std::{
    cell::OnceCell,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

use eframe::egui;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationWillFinishLaunchingNotification, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject,
    NSObjectProtocol, NSString, ns_string,
};

pub enum Action {
    Open,
    ExportPng,
    ExportPdf,
    OpenDocument(Result<PathBuf, String>),
}

struct MenuTargetIvars {
    sender: Sender<Action>,
    context: OnceCell<egui::Context>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; this class stays on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NCDiveMenuTarget"]
    #[ivars = MenuTargetIvars]
    struct MenuTarget;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        // SAFETY: NotificationCenter invokes this selector with one notification argument.
        #[unsafe(method(ncdiveWillFinishLaunching:))]
        fn will_finish_launching(&self, _notification: &NSNotification) {
            // AppKit installs its handlers during startup; replace this one before it opens documents.
            // SAFETY: This selector has Apple's two-descriptor signature; NativeEvents retains self.
            unsafe {
                let _: () = msg_send![
                    &*NSAppleEventManager::sharedAppleEventManager(),
                    setEventHandler: self,
                    andSelector: sel!(ncdiveOpenDocuments:withReplyEvent:),
                    forEventClass: u32::from_be_bytes(*b"aevt"),
                    andEventID: u32::from_be_bytes(*b"odoc")
                ];
            }
        }

        // SAFETY: AppKit sends this action with its NSMenuItem as the sole argument.
        #[unsafe(method(ncdiveMenuAction:))]
        fn menu_action(&self, item: &NSMenuItem) {
            let action = match item.tag() {
                0 => Action::Open,
                1 => Action::ExportPng,
                2 => Action::ExportPdf,
                _ => return,
            };
            self.send(action);
        }

        // SAFETY: Apple event handlers receive the event and its reply descriptor.
        #[unsafe(method(ncdiveOpenDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            self.send(Action::OpenDocument(document_path(event)));
        }
    }
);

impl MenuTarget {
    fn send(&self, action: Action) {
        let _ = self.ivars().sender.send(action);
        if let Some(context) = self.ivars().context.get() {
            context.request_repaint();
        }
    }
}

fn document_path(event: &NSAppleEventDescriptor) -> Result<PathBuf, String> {
    // SAFETY: AEKeyword is a four-byte code; this selector returns an optional descriptor.
    let files: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, paramDescriptorForKeyword: u32::from_be_bytes(*b"----")] };
    let files = files.ok_or("Finder did not provide a file to open.")?;
    if files.numberOfItems() != 1 {
        return Err("Please open one NetCDF file at a time.".into());
    }
    files
        .descriptorAtIndex(1)
        .and_then(|file| file.fileURLValue())
        .filter(|url| url.isFileURL())
        .and_then(|url| url.to_file_path())
        .ok_or_else(|| "Could not read the file path sent by Finder.".into())
}

pub struct NativeEvents {
    target: Retained<MenuTarget>,
    receiver: Receiver<Action>,
}

impl NativeEvents {
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("The application starts on the main thread");
        NSApplication::sharedApplication(mtm);
        let (sender, receiver) = mpsc::channel();
        let target = MenuTarget::alloc(mtm).set_ivars(MenuTargetIvars {
            sender,
            context: OnceCell::new(),
        });
        // SAFETY: NSObject's init returns the initialized instance of this subclass.
        let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
        // SAFETY: The selector takes one notification; NativeEvents retains the observer.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &target,
                sel!(ncdiveWillFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
            );
        }
        Self { target, receiver }
    }
}

impl Drop for NativeEvents {
    fn drop(&mut self) {
        // SAFETY: Remove our handler before releasing its target; both event codes are UInt32.
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(&self.target);
            let _: () = msg_send![
                &*NSAppleEventManager::sharedAppleEventManager(),
                removeEventHandlerForEventClass: u32::from_be_bytes(*b"aevt"),
                andEventID: u32::from_be_bytes(*b"odoc")
            ];
        }
    }
}

pub struct FileMenu {
    main: Retained<NSMenu>,
    root: Retained<NSMenuItem>,
    items: [Retained<NSMenuItem>; 3],
    events: NativeEvents,
}

impl FileMenu {
    pub fn new(context: &egui::Context, events: NativeEvents) -> Self {
        let mtm = MainThreadMarker::new().expect("The application UI runs on the main thread");
        let main = NSApplication::sharedApplication(mtm)
            .mainMenu()
            .expect("eframe installs the macOS application menu before creating the UI");
        let target = &events.target;
        target.ivars().context.get_or_init(|| context.clone());
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("File"));
        menu.setAutoenablesItems(false);
        let items = [
            ("Open NetCDF file…", "o", 0),
            ("Export PNG…", "", 1),
            ("Export PDF…", "", 2),
        ]
        .map(|(title, key, tag)| {
            // SAFETY: The selector matches MenuTarget's action; FileMenu retains its target.
            let item = unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(sel!(ncdiveMenuAction:)),
                    &NSString::from_str(key),
                );
                item.setTarget(Some(target));
                item
            };
            item.setTag(tag);
            item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
            item.setEnabled(tag == 0);
            item
        });
        menu.addItem(&items[0]);
        // SAFETY: AppKit routes this standard action through the active window's responder chain.
        let close = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Close Window"),
                Some(sel!(performClose:)),
                ns_string!("w"),
            )
        };
        close.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        menu.addItem(&close);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&items[1]);
        menu.addItem(&items[2]);
        let root = NSMenuItem::new(mtm);
        root.setTitle(ns_string!("File"));
        root.setSubmenu(Some(&menu));
        main.addItem(&root);
        debug_assert_eq!(menu.numberOfItems(), 5);
        Self {
            main,
            root,
            items,
            events,
        }
    }

    pub fn take_action(&self) -> Option<Action> {
        self.events.receiver.try_recv().ok()
    }

    pub fn set_enabled(&self, open: bool, export: bool) {
        self.items[0].setEnabled(open);
        self.items[1].setEnabled(export);
        self.items[2].setEnabled(export);
    }
}

impl Drop for FileMenu {
    fn drop(&mut self) {
        self.main.removeItem(&self.root);
        for item in &self.items {
            // SAFETY: Clear the non-owning target before releasing MenuTarget.
            unsafe { item.setTarget(None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSURL;

    fn event(paths: &[&str]) -> Retained<NSAppleEventDescriptor> {
        let event = NSAppleEventDescriptor::recordDescriptor();
        let files = NSAppleEventDescriptor::listDescriptor();
        for (index, path) in paths.iter().enumerate() {
            let url = NSURL::from_file_path(path).unwrap();
            files.insertDescriptor_atIndex(
                &NSAppleEventDescriptor::descriptorWithFileURL(&url),
                index as isize + 1,
            );
        }
        // SAFETY: setParamDescriptor accepts a descriptor and a four-byte AEKeyword.
        unsafe {
            let _: () = msg_send![&*event, setParamDescriptor: &*files, forKeyword: u32::from_be_bytes(*b"----")];
        }
        event
    }

    #[test]
    fn finder_file_paths_preserve_spaces_and_unicode() {
        let path = "/tmp/Ocean data/温度 #1.nc";
        assert_eq!(document_path(&event(&[path])).unwrap(), PathBuf::from(path));
    }

    #[test]
    fn finder_requests_require_one_local_file() {
        assert!(document_path(&NSAppleEventDescriptor::recordDescriptor()).is_err());
        assert!(document_path(&event(&[])).is_err());
        assert!(document_path(&event(&["/tmp/a.nc", "/tmp/b.nc"])).is_err());
    }
}
