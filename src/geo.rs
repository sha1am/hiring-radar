/// Working out where a job actually is, and whether that is somewhere you want.
///
/// Two problems, and the second is the subtle one.
///
/// 1. Job boards report a location field; feed posts do not. A "we're hiring!"
///    post says "our Bengaluru team" mid-paragraph, or nothing at all. Without
///    inference those posts have no location, so they can neither be displayed
///    nor filtered, and "only send me India jobs" silently passes every one.
///
/// 2. Raw substring matching against the configured list does not work. You
///    write "Gurugram" and the post says "Gurgaon"; you write "India" and the
///    post says "Bengaluru". Both are misses, and under a hard filter both throw
///    away exactly the jobs you asked for. So place names are canonicalised on
///    both sides, and a country matches any city inside it.
///
/// Deliberately a fixed gazetteer rather than anything clever: geocoding would
/// be a network call on the instant path for every post, and the set of places
/// a person filters on is small and stable.

/// (needle, canonical name, country code, is_country).
///
/// Order matters: longer and more specific forms first, so "new delhi" does not
/// report as "delhi" and a city always wins over its country.
const PLACES: &[(&str, &str, &str, bool)] = &[
    // --- India: NCR ---
    ("new delhi", "New Delhi", "IN", false),
    ("delhi ncr", "Delhi NCR", "IN", false),
    ("gurugram", "Gurugram", "IN", false),
    ("gurgaon", "Gurugram", "IN", false),
    ("noida", "Noida", "IN", false),
    ("faridabad", "Faridabad", "IN", false),
    ("ghaziabad", "Ghaziabad", "IN", false),
    ("delhi", "Delhi", "IN", false),
    // --- India: other metros ---
    ("bengaluru", "Bengaluru", "IN", false),
    ("bangalore", "Bengaluru", "IN", false),
    ("hyderabad", "Hyderabad", "IN", false),
    ("secunderabad", "Hyderabad", "IN", false),
    ("navi mumbai", "Navi Mumbai", "IN", false),
    ("mumbai", "Mumbai", "IN", false),
    ("bombay", "Mumbai", "IN", false),
    ("pune", "Pune", "IN", false),
    ("chennai", "Chennai", "IN", false),
    ("kolkata", "Kolkata", "IN", false),
    ("ahmedabad", "Ahmedabad", "IN", false),
    ("jaipur", "Jaipur", "IN", false),
    ("indore", "Indore", "IN", false),
    ("chandigarh", "Chandigarh", "IN", false),
    ("kochi", "Kochi", "IN", false),
    ("cochin", "Kochi", "IN", false),
    ("coimbatore", "Coimbatore", "IN", false),
    ("thiruvananthapuram", "Thiruvananthapuram", "IN", false),
    ("trivandrum", "Thiruvananthapuram", "IN", false),
    ("bhubaneswar", "Bhubaneswar", "IN", false),
    ("nagpur", "Nagpur", "IN", false),
    ("mysuru", "Mysuru", "IN", false),
    ("mysore", "Mysuru", "IN", false),
    ("vadodara", "Vadodara", "IN", false),
    ("surat", "Surat", "IN", false),
    ("lucknow", "Lucknow", "IN", false),
    ("bharat", "India", "IN", true),
    ("india", "India", "IN", true),
    // --- global hubs, so a filtered-out post still says where it is ---
    ("san francisco", "San Francisco", "US", false),
    ("bay area", "Bay Area", "US", false),
    ("new york", "New York", "US", false),
    ("seattle", "Seattle", "US", false),
    ("austin", "Austin", "US", false),
    ("boston", "Boston", "US", false),
    ("chicago", "Chicago", "US", false),
    ("los angeles", "Los Angeles", "US", false),
    ("united states", "United States", "US", true),
    ("toronto", "Toronto", "CA", false),
    ("vancouver", "Vancouver", "CA", false),
    ("canada", "Canada", "CA", true),
    ("london", "London", "GB", false),
    ("united kingdom", "United Kingdom", "GB", true),
    ("dublin", "Dublin", "IE", false),
    ("ireland", "Ireland", "IE", true),
    ("berlin", "Berlin", "DE", false),
    ("munich", "Munich", "DE", false),
    ("germany", "Germany", "DE", true),
    ("amsterdam", "Amsterdam", "NL", false),
    ("netherlands", "Netherlands", "NL", true),
    ("paris", "Paris", "FR", false),
    ("zurich", "Zurich", "CH", false),
    ("stockholm", "Stockholm", "SE", false),
    ("warsaw", "Warsaw", "PL", false),
    ("warszawa", "Warsaw", "PL", false),
    ("lisbon", "Lisbon", "PT", false),
    ("madrid", "Madrid", "ES", false),
    ("barcelona", "Barcelona", "ES", false),
    ("tel aviv", "Tel Aviv", "IL", false),
    ("dubai", "Dubai", "AE", false),
    ("singapore", "Singapore", "SG", true),
    ("tokyo", "Tokyo", "JP", false),
    ("sydney", "Sydney", "AU", false),
    ("melbourne", "Melbourne", "AU", false),
    ("australia", "Australia", "AU", true),
    ("hong kong", "Hong Kong", "HK", true),
    ("shanghai", "Shanghai", "CN", false),
    ("beijing", "Beijing", "CN", false),
    ("seoul", "Seoul", "KR", false),
    ("sao paulo", "Sao Paulo", "BR", false),
    ("mexico city", "Mexico City", "MX", false),
];

const REMOTE_HINTS: &[&str] = &[
    "fully remote",
    "100% remote",
    "work from home",
    "work from anywhere",
    "remote-first",
    "remote first",
    "remote",
    "anywhere",
    "wfh",
];

/// A resolved place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: &'static str,
    pub country: &'static str,
    pub is_country: bool,
}

pub use crate::text::contains_word;

/// First place mentioned in free text.
pub fn lookup(text: &str) -> Option<Place> {
    let hay = text.to_lowercase();
    PLACES
        .iter()
        .find(|(needle, _, _, _)| contains_word(&hay, needle))
        .map(|(_, name, country, is_country)| Place {
            name,
            country,
            is_country: *is_country,
        })
}

/// Resolve a location the *user* typed into a canonical place, so "gurgaon",
/// "Gurugram" and "GURGAON" all mean the same thing as the post that says any
/// of them.
pub fn canonical(term: &str) -> Option<Place> {
    let t = term.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    PLACES
        .iter()
        .find(|(needle, _, _, _)| *needle == t)
        .map(|(_, name, country, is_country)| Place {
            name,
            country,
            is_country: *is_country,
        })
        .or_else(|| lookup(&t))
}

/// Does a post located at `place` satisfy a wanted location?
///
/// A country matches every city inside it — listing "India" has to match a post
/// in Bengaluru, or the filter rejects precisely what it was set up to keep.
pub fn satisfies(place: Place, wanted: Place) -> bool {
    if wanted.is_country {
        return place.country == wanted.country;
    }
    place.name == wanted.name
}

/// Best-effort place name for display.
pub fn infer(text: &str) -> Option<String> {
    lookup(text).map(|p| p.name.to_string())
}

/// Whether the text advertises remote work.
pub fn is_remote(text: &str) -> bool {
    let hay = text.to_lowercase();
    REMOTE_HINTS.iter().any(|h| contains_word(&hay, h))
}

/// A displayable location for a post that has none. `None` means we genuinely
/// could not tell, which the filter treats as its own case rather than as a miss.
pub fn describe(text: &str) -> Option<String> {
    match (infer(text), is_remote(text)) {
        (Some(p), true) => Some(format!("{p} · remote")),
        (Some(p), false) => Some(p),
        (None, true) => Some("Remote".to_string()),
        (None, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_indian_cities_in_prose() {
        assert_eq!(infer("our Bengaluru team is growing").as_deref(), Some("Bengaluru"));
        assert_eq!(infer("based out of Gurgaon").as_deref(), Some("Gurugram"));
    }

    #[test]
    fn normalises_aliases() {
        assert_eq!(infer("bangalore").as_deref(), Some("Bengaluru"));
        assert_eq!(infer("Bombay").as_deref(), Some("Mumbai"));
        assert_eq!(infer("Trivandrum").as_deref(), Some("Thiruvananthapuram"));
    }

    #[test]
    fn prefers_the_city_over_the_country_and_the_specific_over_the_general() {
        assert_eq!(infer("Pune, India").as_deref(), Some("Pune"));
        assert_eq!(infer("New Delhi").as_deref(), Some("New Delhi"));
        assert_eq!(infer("Navi Mumbai").as_deref(), Some("Navi Mumbai"));
    }

    /// Substring matching would make these false positives, and both are words
    /// that genuinely turn up in hiring posts.
    #[test]
    fn respects_word_boundaries() {
        assert_eq!(infer("contact Puneet for details"), None);
        assert_eq!(infer("our Indiana office"), None);
    }

    /// The pair that motivated canonicalisation: you type one spelling, the post
    /// uses the other.
    #[test]
    fn alias_on_either_side_still_matches() {
        let wanted = canonical("Gurugram").unwrap();
        let post = lookup("hiring in Gurgaon").unwrap();
        assert!(satisfies(post, wanted));

        let wanted = canonical("bangalore").unwrap();
        let post = lookup("our Bengaluru office").unwrap();
        assert!(satisfies(post, wanted));
    }

    /// The rule that makes "only India" usable at all.
    #[test]
    fn a_country_matches_its_cities() {
        let india = canonical("India").unwrap();
        assert!(india.is_country);
        for city in ["Bengaluru", "Gurgaon", "Hyderabad", "Pune", "Noida"] {
            let p = lookup(city).unwrap_or_else(|| panic!("{city} should resolve"));
            assert!(satisfies(p, india), "{city} should count as India");
        }
        let sf = lookup("San Francisco").unwrap();
        assert!(!satisfies(sf, india), "San Francisco is not India");
    }

    #[test]
    fn a_city_does_not_match_a_different_city() {
        let blr = canonical("Bengaluru").unwrap();
        assert!(!satisfies(lookup("Pune").unwrap(), blr));
    }

    #[test]
    fn detects_remote() {
        assert!(is_remote("This is a fully remote role"));
        assert!(!is_remote("we are in the office five days"));
    }

    #[test]
    fn describe_combines_place_and_remote() {
        assert_eq!(
            describe("Remote role, Bengaluru preferred").as_deref(),
            Some("Bengaluru · remote")
        );
        assert_eq!(describe("fully remote").as_deref(), Some("Remote"));
        assert_eq!(describe("we are hiring engineers"), None);
    }
}
