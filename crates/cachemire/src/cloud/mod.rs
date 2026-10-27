//! The title bar's cloud account (AWS): the Cloud button signs in or out, the profile
//! button picks the local profile, and its ElastiCache connections show in the
//! connection dropdown (see `cachemire_core::aws`).

use crate::error::Localize;
use rustc_hash::FxHashSet;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::UserAttentionType;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::AwsAccount;
use crate::MainWindow;
use crate::app::App;
use cachemire_core::aws;
use rust_i18n::t;

pub mod settings;

/// Whether the cloud account is signed in, and with which profile.
#[derive(Default)]
pub struct Cloud {
    connected: bool,
    profile: String,
    // bumped whenever the account or profile changes: a listing still on its way is then stale
    generation: u64,
    // the last session's connection, reached once signed in and listed (see `resume`)
    resume: Option<Resume>,
}

impl Cloud {
    /// The profile whose connections are listed ("" for none).
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

/// A cloud connection to get back to: its profile, its id, and the connect attempt
/// counted when it was asked for (one made since by the user wins).
struct Resume {
    profile: String,
    connection: String,
    attempt: u64,
}

/// Signs in (in the browser when the session has expired), lists `profile`'s
/// connections and connects to `connection`, as if picked one after the other.
pub fn resume(app: &App, window: &MainWindow, profile: String, connection: String) {
    if !window.global::<AwsAccount>().get_available() || app.cloud.borrow().connected {
        return;
    }
    let attempt = app.state.connect_attempt.load(Ordering::SeqCst);
    app.cloud.borrow_mut().resume = Some(Resume { profile, connection, attempt });
    window.global::<AwsAccount>().invoke_clicked();
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

/// The page the browser shows once an AWS sign-in there is over.
fn sign_in_page() -> aws::SignInPage {
    aws::SignInPage {
        title: "Cachemire".to_string(),
        signed_in: t!("You're signed in to AWS. You can close this tab and go back to Cachemire.").into_owned(),
        refused: t!("The AWS sign-in didn't go through. You can close this tab and try again from Cachemire.").into_owned(),
    }
}

/// Forgets the account's connections, dropping the one in use if it is one of them.
fn clear_connections(app: &App, window: &MainWindow) {
    if window.connection_id().starts_with(aws::ID_PREFIX) {
        crate::connections::disconnect(app, window);
    }
    app.state.connections.lock().set_cloud_connections("", "", Vec::new());
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
            let page = sign_in_page();
            app.rt.spawn(async move {
                let result = aws::login(&page).await;
                let names: Vec<slint::SharedString> = aws::profiles().await.into_iter().map(|p| p.name.into()).collect();
                crate::app::later(move |app, window| {
                    window.global::<AwsAccount>().set_busy(false);
                    bring_to_front(window);
                    match result {
                        Ok(()) => {
                            tracing::info!(profiles = names.len(), "signed in to AWS");
                            let resumed = {
                                let mut cloud = app.cloud.borrow_mut();
                                cloud.resume = cloud.resume.take().filter(|r| names.iter().any(|n| n.as_str() == r.profile));
                                cloud.resume.as_ref().map(|r| r.profile.clone())
                            };
                            window.global::<AwsAccount>().set_profiles(ModelRc::new(VecModel::from(names)));
                            app.cloud.borrow_mut().connected = true;
                            window.global::<AwsAccount>().set_connected(true);
                            if let Some(profile) = resumed {
                                window.global::<AwsAccount>().invoke_profile_selected(profile.into());
                            }
                        }
                        Err(e) => {
                            tracing::warn!("aws sign-in failed: {e}");
                            app.cloud.borrow_mut().resume = None;
                            window.global::<AwsAccount>().set_error(e.localized().into());
                        }
                    }
                });
            });
        }
    });

    window.global::<AwsAccount>().on_profile_selected({
        let app = app.clone();
        move |name| {
            let Some(window) = app.window.upgrade() else { return };
            let name = name.to_string();
            {
                let mut cloud = app.cloud.borrow_mut();
                if cloud.resume.as_ref().is_some_and(|r| r.profile != name) {
                    cloud.resume = None;
                }
                if cloud.profile == name {
                    return;
                }
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

            let page = sign_in_page();
            app.rt.spawn(async move {
                let result = aws::redis_connections(&name, &page).await;
                crate::app::later(move |app, window| {
                    if app.cloud.borrow().generation != generation {
                        return;
                    }
                    let state = &app.state;
                    window.global::<AwsAccount>().set_busy(false);
                    bring_to_front(window);
                    match result {
                        Ok(aws::Listing { account, region, mut items }) => {
                            tracing::info!(%account, %region, endpoints = items.len(), "listed the cloud connections");
                            // a complete listing: counts the endpoints with settings that it lacks
                            let present: FxHashSet<String> = items.iter().map(|item| item.name.clone()).collect();
                            let cleanups = {
                                let mut settings = state.settings.lock();
                                let cleanups = settings.observe_cloud_listing(&account, &region, &present).unwrap_or_else(|e| {
                                    tracing::error!("pruning the cloud settings failed: {e}");
                                    Vec::new()
                                });
                                settings.apply_cloud(&account, &region, &mut items);
                                cleanups
                            };
                            app.rt.spawn_blocking(move || cleanups.into_iter().for_each(|cleanup| cleanup.run()));
                            state.connections.lock().set_cloud_connections(&account, &region, items);
                            window.global::<AwsAccount>().set_account_known(true);
                            crate::connections::refresh(app, window);
                            // the last session's connection, unless one was picked since
                            let resume = app.cloud.borrow_mut().resume.take().filter(|r| {
                                r.attempt == state.connect_attempt.load(Ordering::SeqCst)
                                    && state.connections.lock().get(&r.connection).is_some()
                            });
                            if let Some(resume) = resume {
                                crate::connect_to(window, state.clone(), &app.rt, resume.connection, true);
                            }
                        }
                        Err(e) => {
                            app.cloud.borrow_mut().resume = None;
                            tracing::warn!("listing the cloud connections failed: {e}");
                            window.global::<AwsAccount>().set_error(e.localized().into());
                        }
                    }
                });
            });
        }
    });
}
