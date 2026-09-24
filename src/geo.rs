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
    // --- Gulf ---
    // The single largest destination for Indian engineers after India itself,
    // and it had exactly one entry (Dubai) until this list existed. A region
    // filter that can't see Riyadh or Doha is a region filter in name only.
    ("abu dhabi", "Abu Dhabi", "AE", false),
    ("dubai", "Dubai", "AE", false),
    ("sharjah", "Sharjah", "AE", false),
    ("united arab emirates", "United Arab Emirates", "AE", true),
    ("uae", "United Arab Emirates", "AE", true),
    ("riyadh", "Riyadh", "SA", false),
    ("jeddah", "Jeddah", "SA", false),
    ("dammam", "Dammam", "SA", false),
    ("neom", "NEOM", "SA", false),
    ("saudi arabia", "Saudi Arabia", "SA", true),
    ("ksa", "Saudi Arabia", "SA", true),
    ("doha", "Doha", "QA", false),
    ("qatar", "Qatar", "QA", true),
    ("kuwait city", "Kuwait City", "KW", false),
    ("kuwait", "Kuwait", "KW", true),
    ("manama", "Manama", "BH", false),
    ("bahrain", "Bahrain", "BH", true),
    ("muscat", "Muscat", "OM", false),
    ("oman", "Oman", "OM", true),
    // --- Southeast Asia ---
    ("singapore", "Singapore", "SG", true),
    ("kuala lumpur", "Kuala Lumpur", "MY", false),
    ("penang", "Penang", "MY", false),
    ("malaysia", "Malaysia", "MY", true),
    ("jakarta", "Jakarta", "ID", false),
    ("indonesia", "Indonesia", "ID", true),
    ("bangkok", "Bangkok", "TH", false),
    ("thailand", "Thailand", "TH", true),
    ("ho chi minh", "Ho Chi Minh City", "VN", false),
    ("hanoi", "Hanoi", "VN", false),
    ("vietnam", "Vietnam", "VN", true),
    ("manila", "Manila", "PH", false),
    ("philippines", "Philippines", "PH", true),
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

/// Coarse regions, for the "where in the world" filter.
///
/// Deliberately coarse and deliberately closed. The question this answers is
/// the one you actually ask when scanning — India, the Gulf, Singapore, or
/// somewhere that needs a visa conversation — not "which country". A per-country
/// filter would be forty chips, most with one row behind them.
///
/// A country the gazetteer knows but that isn't in any bucket falls to `other`,
/// which is honest: it is somewhere, we know where, and it isn't one of the
/// places you sort by. A place we can't resolve at all gets no region, and the
/// filter leaves those alone rather than hiding them.
pub const REGIONS: &[&str] = &[
    "india", "gulf", "sea", "apac", "europe", "americas", "other",
];

const REGION_OF: &[(&str, &[&str])] = &[
    ("india", &["IN"]),
    // The GCC states. Grouped because they hire on the same terms and you would
    // consider them as one set, not one at a time.
    ("gulf", &["AE", "SA", "QA", "KW", "BH", "OM"]),
    ("sea", &["SG", "MY", "ID", "TH", "VN", "PH"]),
    ("apac", &["JP", "AU", "NZ", "HK", "CN", "KR", "TW"]),
    (
        "europe",
        &[
            "GB", "IE", "DE", "NL", "FR", "CH", "SE", "NO", "DK", "FI", "PL", "PT", "ES", "IT",
            "CZ", "RO", "AT", "BE",
        ],
    ),
    ("americas", &["US", "CA", "MX", "BR", "AR", "CL"]),
];

/// Which region a country code belongs to. Never None for a code the gazetteer
/// produced — unclassified countries land in "other".
pub fn region(country: &str) -> &'static str {
    let cc = country.trim().to_uppercase();
    REGION_OF
        .iter()
        .find(|(_, codes)| codes.iter().any(|c| *c == cc))
        .map(|(name, _)| *name)
        .unwrap_or("other")
}

/// The region a piece of text is about, if any place can be resolved from it.
///
/// `None` means we could not tell where the job is — which is its own case, not
/// a miss. Feed posts frequently never name a city, and a region filter that
/// swallowed those would hide real matches for how the post was written.
pub fn region_of(text: &str) -> Option<&'static str> {
    lookup(text).map(|p| region(p.country))
}

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
    fn the_gulf_is_more_than_dubai() {
        // It was exactly Dubai before the region filter existed, which made
        // "show me Gulf jobs" quietly mean "show me Dubai jobs".
        for city in [
            "Riyadh",
            "Doha",
            "Abu Dhabi",
            "Manama",
            "Muscat",
            "Kuwait City",
        ] {
            assert_eq!(region_of(city), Some("gulf"), "{city} is not in the Gulf");
        }
    }

    #[test]
    fn regions_cover_the_places_people_actually_consider() {
        assert_eq!(region_of("Bengaluru, India"), Some("india"));
        assert_eq!(region_of("Singapore"), Some("sea"));
        assert_eq!(region_of("Kuala Lumpur"), Some("sea"));
        assert_eq!(region_of("Berlin, Germany"), Some("europe"));
        assert_eq!(region_of("London"), Some("europe"));
        assert_eq!(region_of("Seattle, WA"), Some("americas"));
        assert_eq!(region_of("Tokyo"), Some("apac"));
    }

    #[test]
    fn a_known_country_outside_every_bucket_is_other_not_nothing() {
        // Tel Aviv is somewhere, we know where, and it is not a bucket you sort
        // by. "other" is the honest answer; None would mean "we could not tell".
        assert_eq!(region_of("Tel Aviv"), Some("other"));
    }

    #[test]
    fn an_unresolvable_location_has_no_region_at_all() {
        // A feed post that never names a place must not be filed under a region,
        // or a region filter hides it for how the post was written.
        assert_eq!(region_of("fully remote, EMEA timezone"), None);
        assert_eq!(region_of(""), None);
    }

    #[test]
    fn every_region_name_is_one_the_filter_offers() {
        for (name, _) in REGION_OF {
            assert!(REGIONS.contains(name), "{name} is produced but not offered");
        }
        assert!(REGIONS.contains(&"other"));
    }

    #[test]
    fn uae_spellings_all_land_in_the_gulf() {
        assert_eq!(region_of("UAE"), Some("gulf"));
        assert_eq!(region_of("United Arab Emirates"), Some("gulf"));
        assert_eq!(region_of("Dubai, UAE"), Some("gulf"));
    }

    #[test]
    fn finds_indian_cities_in_prose() {
        assert_eq!(
            infer("our Bengaluru team is growing").as_deref(),
            Some("Bengaluru")
        );
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
