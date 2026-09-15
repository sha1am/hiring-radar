use crate::text::contains_word;

/// What technologies a post is about.
///
/// The radar's text search already lets you type "golang", but typing is a poor
/// fit for scanning: you want to flip between "show me Go" and "show me Go or
/// Rust" without composing queries. That needs the technologies as *data* —
/// extracted once at ingest, stored on the row, and offered as chips.
///
/// A curated list rather than anything learned, for the same reason the
/// gazetteer is: the set of things a backend engineer filters on is small,
/// stable, and full of aliases that no amount of cleverness would guess
/// ("k8s" is Kubernetes, "gcp" is Google Cloud, "postgres" and "psql" and
/// "postgresql" are one thing).

/// (needle, canonical label). Multiple needles map to one label; the label is
/// what gets stored and what the chip shows.
const TECH: &[(&str, &str)] = &[
    // --- languages ---
    ("golang", "Go"),
    ("go lang", "Go"),
    (" go ", "Go"), // bare "go" is too common; handled specially below
    ("rust", "Rust"),
    ("python", "Python"),
    ("java", "Java"),
    ("kotlin", "Kotlin"),
    ("scala", "Scala"),
    ("typescript", "TypeScript"),
    ("javascript", "JavaScript"),
    ("c++", "C++"),
    ("c#", "C#"),
    ("ruby", "Ruby"),
    ("php", "PHP"),
    ("elixir", "Elixir"),
    ("clojure", "Clojure"),
    // --- runtimes / frameworks ---
    ("node.js", "Node"),
    ("nodejs", "Node"),
    ("django", "Django"),
    // Go's web frameworks, which are also the clearest signal a posting that
    // never writes the word "Golang" is a Go job.
    ("gin", "Gin"),
    ("gorm", "GORM"),
    ("fastapi", "FastAPI"),
    ("flask", "Flask"),
    ("spring boot", "Spring"),
    ("spring", "Spring"),
    ("rails", "Rails"),
    ("react", "React"),
    ("next.js", "Next.js"),
    // --- data ---
    ("postgresql", "Postgres"),
    ("postgres", "Postgres"),
    ("psql", "Postgres"),
    ("mysql", "MySQL"),
    ("mongodb", "MongoDB"),
    ("mongo", "MongoDB"),
    ("cassandra", "Cassandra"),
    ("dynamodb", "DynamoDB"),
    ("redis", "Redis"),
    ("elasticsearch", "Elasticsearch"),
    ("opensearch", "Elasticsearch"),
    ("clickhouse", "ClickHouse"),
    ("snowflake", "Snowflake"),
    ("bigquery", "BigQuery"),
    // --- streaming / pipelines ---
    ("kafka", "Kafka"),
    ("pulsar", "Pulsar"),
    ("rabbitmq", "RabbitMQ"),
    ("airflow", "Airflow"),
    ("spark", "Spark"),
    ("flink", "Flink"),
    // --- infra ---
    ("kubernetes", "Kubernetes"),
    ("k8s", "Kubernetes"),
    ("docker", "Docker"),
    ("terraform", "Terraform"),
    ("ansible", "Ansible"),
    ("aws", "AWS"),
    ("amazon web services", "AWS"),
    ("gcp", "GCP"),
    ("google cloud", "GCP"),
    ("azure", "Azure"),
    ("prometheus", "Prometheus"),
    ("grafana", "Grafana"),
    ("kibana", "Kibana"),
    // --- shapes of work ---
    ("microservices", "Microservices"),
    ("microservice", "Microservices"),
    ("distributed systems", "Distributed systems"),
    ("event driven", "Event-driven"),
    ("event-driven", "Event-driven"),
    ("graphql", "GraphQL"),
    ("grpc", "gRPC"),
    ("rest api", "REST"),
    ("restful", "REST"),
    ("machine learning", "ML"),
    ("deep learning", "ML"),
    ("llm", "LLM"),
    ("genai", "LLM"),
];

/// Bare "go" is a word in English, so it only counts with company: a post that
/// says "go" AND something Go-flavoured. Without this, every "go-getter" and
/// "ready to go" post is tagged Go.
const GO_COMPANIONS: &[&str] = &[
    "golang", "goroutine", "gin", "fiber", "gorm", "go developer", "go engineer",
    "go backend", "in go", "go services", "go microservices",
];

/// Extract the technologies a post mentions, as canonical labels.
pub fn extract(text: &str) -> Vec<String> {
    let hay = format!(" {} ", text.to_lowercase());
    let mut out: Vec<String> = Vec::new();

    for (needle, label) in TECH {
        // The bare-go entry is a marker for the special case below.
        if *needle == " go " {
            continue;
        }
        if contains_word(&hay, needle) && !out.iter().any(|t| t == label) {
            out.push((*label).to_string());
        }
    }

    // Go, carefully.
    if !out.iter().any(|t| t == "Go") {
        let go_word = contains_word(&hay, "go");
        let companion = GO_COMPANIONS.iter().any(|c| hay.contains(c));
        if go_word && companion {
            out.push("Go".to_string());
        }
    }

    out.sort();
    out
}

/// The labels that name a *language*, as opposed to a database, a cloud or a
/// framework.
///
/// The distinction matters for the stack gate: "this job is Ruby and I write
/// Go" is a real reason to skip a posting, while "this job uses MySQL and I use
/// Postgres" is not — you would learn MySQL on the Monday. So only languages
/// get a vote on whether a post is about your stack at all.
pub const LANGUAGES: &[&str] = &[
    "Go", "Rust", "Python", "Java", "Kotlin", "Scala", "TypeScript", "JavaScript",
    "C++", "C#", "Ruby", "PHP", "Elixir", "Clojure",
];

pub fn is_language(label: &str) -> bool {
    LANGUAGES.iter().any(|l| l.eq_ignore_ascii_case(label.trim()))
}

/// Just the languages a post names.
pub fn languages(tags: &[String]) -> Vec<String> {
    tags.iter().filter(|t| is_language(t)).cloned().collect()
}

/// Storage form: comma-delimited with leading and trailing commas, so a SQL
/// `instr(tags, ',Go,')` cannot match "Golang" or "Django" by accident.
pub fn encode(tags: &[String]) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    Some(format!(",{},", tags.join(",")))
}

/// Back to a list for display.
pub fn decode(stored: &str) -> Vec<String> {
    stored
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The value a SQL `instr` test needs for one tag.
pub fn needle(tag: &str) -> String {
    format!(",{},", tag.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_obvious_ones() {
        let t = extract("We need a backend engineer: Golang, Kafka, PostgreSQL, Kubernetes on AWS.");
        for want in ["Go", "Kafka", "Postgres", "Kubernetes", "AWS"] {
            assert!(t.contains(&want.to_string()), "missing {want} in {t:?}");
        }
    }

    #[test]
    fn collapses_aliases_to_one_label() {
        assert_eq!(extract("k8s and kubernetes"), vec!["Kubernetes"]);
        assert_eq!(extract("postgres, postgresql, psql"), vec!["Postgres"]);
        assert_eq!(extract("nodejs and node.js"), vec!["Node"]);
    }

    /// The failure that makes a Go chip useless: English.
    #[test]
    fn bare_go_needs_corroboration() {
        assert!(!extract("ready to go and a real go-getter").contains(&"Go".to_string()));
        assert!(!extract("here we go again").contains(&"Go".to_string()));
        assert!(extract("strong experience in Go and goroutines").contains(&"Go".to_string()));
        assert!(extract("Go developer wanted, gin framework").contains(&"Go".to_string()));
    }

    /// Substring matching would tag Django as Go and Java as JavaScript.
    #[test]
    fn respects_word_boundaries() {
        let t = extract("Django and Java only");
        assert!(t.contains(&"Django".to_string()));
        assert!(t.contains(&"Java".to_string()));
        assert!(!t.contains(&"Go".to_string()));
        assert!(!t.contains(&"JavaScript".to_string()));
    }

    #[test]
    fn keeps_symbol_names_intact() {
        let t = extract("C++ and C# roles");
        assert!(t.contains(&"C++".to_string()));
        assert!(t.contains(&"C#".to_string()));
    }

    #[test]
    fn encoding_cannot_prefix_match() {
        let enc = encode(&["Go".into(), "Kafka".into()]).unwrap();
        assert!(enc.contains(&needle("Go")));
        // The whole point of the delimiters.
        assert!(!encode(&["Golang".into()]).unwrap().contains(&needle("Go")));
    }

    #[test]
    fn nothing_found_is_none_not_empty_string() {
        assert_eq!(encode(&extract("we are hiring a designer")), None);
    }
}
