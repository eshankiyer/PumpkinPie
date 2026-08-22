use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_util::text::click::ClickEvent;
use pumpkin_util::text::hover::HoverEvent;
use pumpkin_util::text::{TextComponent, color::NamedColor};
use pumpkin_util::translation::get_translation_text;
use serde::Deserialize;
use std::borrow::Cow;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::command::CommandResult;
use crate::command::{CommandExecutor, CommandSender, args::ConsumedArgs, tree::CommandTree};

const NAMES: [&str; 4] = ["pumpkinpie", "pumpkin", "version", "ver"];

const DESCRIPTION: &str = "Display information about PumpkinPie.";

const CACHE_DURATION: Duration = Duration::from_hours(24);

struct Executor;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("GIT_HASH");
const GIT_HASH_FULL: &str = env!("GIT_HASH_FULL");

#[derive(Deserialize, Clone)]
struct Contributor {
    login: String,
}

struct ContributorCache {
    fetched_at: Instant,
    data: Vec<Contributor>,
}

static CONTRIBUTORS_CACHE: LazyLock<Mutex<Option<ContributorCache>>> =
    LazyLock::new(|| Mutex::new(None));

fn fetch_all_contributors_cached() -> Vec<Contributor> {
    if let Ok(guard) = CONTRIBUTORS_CACHE.lock()
        && let Some(cache) = guard.as_ref()
        && cache.fetched_at.elapsed() < CACHE_DURATION
    {
        return cache.data.clone();
    }

    let contributors = fetch_all_contributors();
    if !contributors.is_empty() {
        if let Ok(mut guard) = CONTRIBUTORS_CACHE.lock() {
            *guard = Some(ContributorCache {
                fetched_at: Instant::now(),
                data: contributors.clone(),
            });
        }
    } else if let Ok(guard) = CONTRIBUTORS_CACHE.lock()
        && let Some(cache) = guard.as_ref()
    {
        return cache.data.clone();
    }

    contributors
}

fn fetch_all_contributors() -> Vec<Contributor> {
    let mut all_contributors = Vec::new();
    let mut next_url = Some(
        "https://api.github.com/repos/eshankiyer/PumpkinPie/contributors?per_page=100".to_string(),
    );

    while let Some(url) = next_url {
        let response = ureq::get(&url).header("User-Agent", "PumpkinPie").call();

        match response {
            Ok(mut res) => {
                if let Ok(contributors) = res.body_mut().read_json::<Vec<Contributor>>() {
                    all_contributors.extend(contributors);
                } else {
                    break;
                }
                let link_header = res.headers().get("link").map(|s| s.to_str().unwrap_or(""));

                next_url = link_header.and_then(extract_next_url);
            }
            Err(_) => break,
        }
    }

    if all_contributors.is_empty() {
        return vec![];
    }

    all_contributors
}

fn extract_next_url(header: &str) -> Option<String> {
    header
        .split(',')
        .find(|part| part.contains("rel=\"next\""))
        .and_then(|part| {
            let start = part.find('<')? + 1;
            let end = part.find('>')?;
            Some(part[start..end].to_string())
        })
}

#[expect(clippy::too_many_lines)]
impl CommandExecutor for Executor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        _server: &'a crate::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let contributors = tokio::task::spawn_blocking(fetch_all_contributors_cached)
                .await
                .unwrap_or_default();
            let contributor_names = contributors
                .iter()
                .map(|c| c.login.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let locale = sender.get_locale();
            let profile = if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            };
            let version_string = format!(
                "{} (Commit: {}/{}) - {} Contributors",
                CARGO_PKG_VERSION,
                GIT_HASH,
                profile,
                contributors.len()
            );
            let mut msg = TextComponent::text("");

            let version_translation = get_translation_text(
                "pumpkin:commands.pumpkin.version",
                locale,
                vec![TextComponent::text(version_string.clone()).0],
            );
            msg = msg.add_child(
                TextComponent::text(version_translation.clone())
                    .hover_event(HoverEvent::show_text(
                        TextComponent::text(format!("Commit: {GIT_HASH_FULL}\n\nContributors:\n"))
                            .add_child(
                                TextComponent::text(contributor_names)
                                    .gradient_named(&[NamedColor::DarkGreen, NamedColor::Green])
                                    .new_line(),
                            ),
                    ))
                    .click_event(ClickEvent::CopyToClipboard {
                        value: Cow::from(version_translation.replace('\n', "")),
                    })
                    .color_named(NamedColor::Green),
            );

            let desc_translation =
                get_translation_text("pumpkin:commands.pumpkin.description", locale, vec![]);
            let desc_hover_translation =
                get_translation_text("pumpkin:commands.pumpkin.description.hover", locale, vec![]);
            msg = msg.add_child(
                TextComponent::text(desc_translation.clone())
                    .click_event(ClickEvent::CopyToClipboard {
                        value: Cow::from(desc_translation.replace('\n', "")),
                    })
                    .hover_event(HoverEvent::show_text(TextComponent::text(
                        desc_hover_translation,
                    )))
                    .color_named(NamedColor::White),
            );

            let mc_version_translation = get_translation_text(
                "pumpkin:commands.pumpkin.minecraft_version",
                locale,
                vec![
                    TextComponent::text(CURRENT_MC_VERSION.to_string()).0,
                    TextComponent::text(CURRENT_MC_VERSION.protocol_version().to_string()).0,
                ],
            );
            let mc_version_hover_translation = get_translation_text(
                "pumpkin:commands.pumpkin.minecraft_version.hover",
                locale,
                vec![],
            );
            msg = msg.add_child(
                TextComponent::text(mc_version_translation.clone())
                    .click_event(ClickEvent::CopyToClipboard {
                        value: Cow::from(mc_version_translation.replace('\n', "")),
                    })
                    .hover_event(HoverEvent::show_text(TextComponent::text(
                        mc_version_hover_translation,
                    )))
                    .color_named(NamedColor::Gold),
            );

            let github_translation =
                get_translation_text("pumpkin:commands.pumpkin.github", locale, vec![]);
            let github_hover_translation =
                get_translation_text("pumpkin:commands.pumpkin.github.hover", locale, vec![]);
            msg = msg.add_child(
                TextComponent::text(github_translation)
                    .click_event(ClickEvent::OpenUrl {
                        url: Cow::from("https://github.com/eshankiyer/PumpkinPie"),
                    })
                    .hover_event(HoverEvent::show_text(TextComponent::text(
                        github_hover_translation,
                    )))
                    .color_named(NamedColor::Blue)
                    .bold()
                    .underlined(),
            );

            msg = msg.add_child(TextComponent::text("  "));

            let website_translation =
                get_translation_text("pumpkin:commands.pumpkin.website", locale, vec![]);
            let website_hover_translation =
                get_translation_text("pumpkin:commands.pumpkin.website.hover", locale, vec![]);
            msg = msg.add_child(
                TextComponent::text(website_translation)
                    .click_event(ClickEvent::OpenUrl {
                        url: Cow::from("https://eshankiyer.github.io/PumpkinPie/"),
                    })
                    .hover_event(HoverEvent::show_text(TextComponent::text(
                        website_hover_translation,
                    )))
                    .color_named(NamedColor::Blue)
                    .bold()
                    .underlined(),
            );

            sender.send_message(msg).await;

            // It makes total sense to return the number of
            // contributors as the i32 result for this command.
            Ok(contributors.len() as i32)
        })
    }
}

pub fn init_command_tree() -> CommandTree {
    CommandTree::new(NAMES, DESCRIPTION).execute(Executor)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cache_duration_is_24_hours() {
        assert_eq!(CACHE_DURATION, Duration::from_hours(24));
    }

    #[test]
    fn contributor_cache_updates_and_retrieves() {
        let mut guard = CONTRIBUTORS_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(ContributorCache {
            fetched_at: Instant::now(),
            data: vec![Contributor {
                login: "test_user".to_string(),
            }],
        });
        drop(guard);

        let contributors = fetch_all_contributors_cached();
        assert_eq!(contributors.len(), 1);
        assert_eq!(contributors[0].login, "test_user");
    }
}
