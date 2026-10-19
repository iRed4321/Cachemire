use super::*;

fn connection(name: &str, profile_id: &str) -> ConnectionItem {
    ConnectionItem {
        id: name.into(),
        name: name.into(),
        connection_url: String::new(),
        username: String::new(),
        password: String::new(),
        ssh: None,
        profile_id: profile_id.into(),
    }
}

#[test]
fn every_color_maps_to_its_own_tint_and_back() {
    for color in [
        ProfileColor::Red,
        ProfileColor::Orange,
        ProfileColor::Yellow,
        ProfileColor::Green,
        ProfileColor::Cyan,
        ProfileColor::Blue,
        ProfileColor::Purple,
        ProfileColor::Pink,
    ] {
        assert_eq!(from_tint(to_tint(color)), color);
    }
}

#[test]
fn a_profile_is_in_use_by_the_connections_that_have_it() {
    let connections = [connection("cache", "prod"), connection("dev", "dev"), connection("queue", "prod"), connection("plain", "")];
    assert_eq!(names_using(&connections, "prod"), vec!["cache".to_string(), "queue".to_string()]);
    assert!(names_using(&connections, "staging").is_empty());
}

#[test]
fn the_message_names_a_few_of_the_connections_and_counts_the_rest() {
    let few = in_use_message(&["cache".to_string(), "queue".to_string()]);
    assert!(few.contains("cache, queue") && !few.contains('+'), "{few}");
    let many = in_use_message(&["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string()]);
    assert!(many.contains("a, b, c +2"), "{many}");
}
