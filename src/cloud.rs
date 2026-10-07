//! The title bar's cloud account (AWS): the Cloud button signs in or out, the profile
//! button picks the local profile, and its ElastiCache connections show in the
//! connection dropdown (see `backend::aws`).

use rustc_hash::FxHashSet;
use std::rc::Rc;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::UserAttentionType;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AwsAccount};
use crate::app::App;
use crate::backend::aws;
use crate::backend::LockExt;
use crate::MainWindow;

/// Whether the cloud account is signed in, and with which profile.
#[derive(Default)]
pub struct Cloud {
    connected: bool,
    profile: String,
    // bumped whenever the account or profile changes: a listing still on its way is then stale
    generation: u64,
}

/// Raises the window and gives it the focus, for when the browser sign-in is done. A window
/// manager may refuse to let a window take the focus: the taskbar entry then asks for attention.
fn bring_to_front(window: &MainWindow) {
    window.window().with_winit_window(|winit_window| {
        winit_window.set_minimized(false);
        winit_window.focus_window();
        if !winit_window.has_focus() {
            winit_window.request_user_attention(Some(UserAttentionType::Informational));
        }
    });
}

/// Forgets the account's connections, dropping the one in use if it is one of them.
fn clear_connections(app: &App, window: &MainWindow) {
    if window.connection_id().starts_with(aws::ID_PREFIX) {
        crate::connections::disconnect(app, window);
    }
    app.state.connections.lock_recover().set_cloud_connections("", "", Vec::new());
    window.global::<AwsAccount>().set_account_known(false);
    crate::connections::refresh(app, window);
}

pub fn init(app: &Rc<App>, window: &MainWindow) {
    window.global::<AwsAccount>().set_available(aws::aws_dir().is_some());

    window.global::<AwsAccount>().on_clicked({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            if window.global::<AwsAccount>().get_busy() {
                return;
            }
            // signed in: the button signs out
            if app.cloud.borrow().connected {
                {
                    let mut cloud = app.cloud.borrow_mut();
                    *cloud = Cloud { generation: cloud.generation + 1, ..Cloud::default() };
                }
                clear_connections(&app, &window);
                window.global::<AwsAccount>().set_connected(false);
                window.global::<AwsAccount>().set_profile("".into());
                window.global::<AwsAccount>().set_error("".into());
                return;
            }

            window.global::<AwsAccount>().set_error("".into());
            window.global::<AwsAccount>().set_busy(true);
            app.rt.spawn(async move {
                let result = aws::login().await;
                let names: Vec<slint::SharedString> = aws::profiles().await.into_iter().map(|p| p.name.into()).collect();
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
                    window.global::<AwsAccount>().set_busy(false);
                    bring_to_front(window);
                    match result {
                        Ok(()) => {
                            window.global::<AwsAccount>().set_profiles(ModelRc::new(VecModel::from(names)));
                            app.cloud.borrow_mut().connected = true;
                            window.global::<AwsAccount>().set_connected(true);
                        }
                        Err(e) => {
                            eprintln!("aws sign-in failed: {e}");
                            window.global::<AwsAccount>().set_error(e.into());
                        }
                    }
                }));
            });
        }
    });

    window.global::<AwsAccount>().on_profile_selected({
        let app = app.clone();
        move |name| {
            let Some(window) = app.window.upgrade() else { return };
            let name = name.to_string();
            if app.cloud.borrow().profile == name {
                return;
            }
            clear_connections(&app, &window);
            let generation = {
                let mut cloud = app.cloud.borrow_mut();
                cloud.profile.clone_from(&name);
                cloud.generation += 1;
                cloud.generation
            };
            window.global::<AwsAccount>().set_profile(name.as_str().into());
            window.global::<AwsAccount>().set_error("".into());
            window.global::<AwsAccount>().set_busy(true);

            app.rt.spawn(async move {
                let result = aws::redis_connections(&name).await;
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
                    if app.cloud.borrow().generation != generation {
                        return;
                    }
                    let state = &app.state;
                    window.global::<AwsAccount>().set_busy(false);
                    bring_to_front(window);
                    match result {
                        Ok(aws::Listing { account, region, mut items }) => {
                            // a complete listing: counts the endpoints with settings that it lacks
                            let present: FxHashSet<String> = items.iter().map(|item| item.name.clone()).collect();
                            let cleanups = {
                                let mut settings = state.settings.lock_recover();
                                let cleanups = settings.observe_cloud_listing(&account, &region, &present).unwrap_or_else(|e| {
                                    eprintln!("pruning the cloud settings failed: {e}");
                                    Vec::new()
                                });
                                settings.apply_cloud(&account, &region, &mut items);
                                cleanups
                            };
                            app.rt.spawn_blocking(move || cleanups.into_iter().for_each(|cleanup| cleanup.run()));
                            state.connections.lock_recover().set_cloud_connections(&account, &region, items);
                            window.global::<AwsAccount>().set_account_known(true);
                            crate::connections::refresh(app, window);
                        }
                        Err(e) => {
                            eprintln!("listing the cloud connections failed: {e}");
                            window.global::<AwsAccount>().set_error(e.into());
                        }
                    }
                }));
            });
        }
    });
}
