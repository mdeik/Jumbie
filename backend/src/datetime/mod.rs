// Backend date/time helpers.
//
// The SSoT for timestamps lives in `jumbie_shared::datetime` and is re-exported
// here so backend call sites keep using `crate::datetime::…`.
//
// Only backend-only helpers live here: `country_to_tz` (needs `chrono-tz`, kept out
// of the shared crate so the wasm bundle never pulls the timezone database) and
// `parse_request_utc` (the strict HTTP-facing wrapper whose `Result<_, String>` shape
// API handlers map to 400 responses).

pub use jumbie_shared::datetime::*;

// Country → IANA timezone. TVDB provides `originalCountry`: US shows map to
// America/New_York, others to the country's capital-city zone. Used to interpret
// TVDB's date-only `aired` field as local midnight, then convert to UTC.

/// Map an ISO 3166-1 alpha-2 or alpha-3 country code to the closest IANA timezone
/// used for TV/streaming premieres in that country.
pub fn country_to_tz(country: &str) -> Option<chrono_tz::Tz> {
    match country.to_lowercase().as_str() {
        // North America
        "us" | "usa" => Some(chrono_tz::America::New_York),
        "ca" | "can" => Some(chrono_tz::America::Toronto),
        "mx" | "mex" => Some(chrono_tz::America::Mexico_City),
        // Europe
        "gb" | "gbr" | "uk" => Some(chrono_tz::Europe::London),
        "fr" | "fra" => Some(chrono_tz::Europe::Paris),
        "de" | "deu" => Some(chrono_tz::Europe::Berlin),
        "es" | "esp" => Some(chrono_tz::Europe::Madrid),
        "it" | "ita" => Some(chrono_tz::Europe::Rome),
        "nl" | "nld" => Some(chrono_tz::Europe::Amsterdam),
        "se" | "swe" => Some(chrono_tz::Europe::Stockholm),
        "no" | "nor" => Some(chrono_tz::Europe::Oslo),
        "dk" | "dnk" => Some(chrono_tz::Europe::Copenhagen),
        "fi" | "fin" => Some(chrono_tz::Europe::Helsinki),
        "pl" | "pol" => Some(chrono_tz::Europe::Warsaw),
        "ru" | "rus" => Some(chrono_tz::Europe::Moscow),
        "ua" | "ukr" => Some(chrono_tz::Europe::Kyiv),
        "tr" | "tur" => Some(chrono_tz::Europe::Istanbul),
        "gr" | "grc" => Some(chrono_tz::Europe::Athens),
        "pt" | "prt" => Some(chrono_tz::Europe::Lisbon),
        "ie" | "irl" => Some(chrono_tz::Europe::Dublin),
        "at" | "aut" => Some(chrono_tz::Europe::Vienna),
        "ch" | "che" => Some(chrono_tz::Europe::Zurich),
        "be" | "bel" => Some(chrono_tz::Europe::Brussels),
        "cz" | "cze" => Some(chrono_tz::Europe::Prague),
        "hu" | "hun" => Some(chrono_tz::Europe::Budapest),
        "ro" | "rou" => Some(chrono_tz::Europe::Bucharest),
        // Asia Pacific
        "jp" | "jpn" => Some(chrono_tz::Asia::Tokyo),
        "kr" | "kor" => Some(chrono_tz::Asia::Seoul),
        "cn" | "chn" => Some(chrono_tz::Asia::Shanghai),
        "in" | "ind" => Some(chrono_tz::Asia::Kolkata),
        "au" | "aus" => Some(chrono_tz::Australia::Sydney),
        "nz" | "nzl" => Some(chrono_tz::Pacific::Auckland),
        "hk" | "hkg" => Some(chrono_tz::Asia::Hong_Kong),
        "tw" | "twn" => Some(chrono_tz::Asia::Taipei),
        "th" | "tha" => Some(chrono_tz::Asia::Bangkok),
        "vn" | "vnm" => Some(chrono_tz::Asia::Ho_Chi_Minh),
        "ph" | "phl" => Some(chrono_tz::Asia::Manila),
        "sg" | "sgp" => Some(chrono_tz::Asia::Singapore),
        "my" | "mys" => Some(chrono_tz::Asia::Kuala_Lumpur),
        "id" | "idn" => Some(chrono_tz::Asia::Jakarta),
        // Latin America
        "br" | "bra" => Some(chrono_tz::America::Sao_Paulo),
        "ar" | "arg" => Some(chrono_tz::America::Argentina::Buenos_Aires),
        "cl" | "chl" => Some(chrono_tz::America::Santiago),
        "co" | "col" => Some(chrono_tz::America::Bogota),
        "pe" | "per" => Some(chrono_tz::America::Lima),
        // Middle East / Africa
        "za" | "zaf" => Some(chrono_tz::Africa::Johannesburg),
        "il" | "isr" => Some(chrono_tz::Asia::Jerusalem),
        "ae" | "are" => Some(chrono_tz::Asia::Dubai),
        "sa" | "sau" => Some(chrono_tz::Asia::Riyadh),
        "eg" | "egy" => Some(chrono_tz::Africa::Cairo),
        "ng" | "nga" => Some(chrono_tz::Africa::Lagos),
        "ke" | "ken" => Some(chrono_tz::Africa::Nairobi),
        _ => None,
    }
}

/// Parse an HTTP request timestamp into UTC — **strict**.
///
/// Requires RFC 3339 with an explicit offset (`Z` or `±HH:MM`); zone-less and
/// date-only input is rejected. Clients state the zone, not us.
///
/// Wrapper over [`parse_rfc3339`] that preserves the `Result<_, String>` error
/// shape API handlers feed into `AppError` / 400 responses, appending the
/// offending input for debugging.
///
/// Values we already own (DB rows, persisted config) use the lenient
/// [`parse_utc`] instead.
pub fn parse_request_utc(s: &str) -> Result<UtcDateTime, String> {
    parse_rfc3339(s).map_err(|e| match e {
        ParseError::Empty => e.to_string(),
        _ => format!("{e}: '{}'", s.trim()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_country_to_tz_us() {
        let tz = country_to_tz("US").unwrap();
        assert_eq!(tz, chrono_tz::America::New_York);
        let tz = country_to_tz("usa").unwrap();
        assert_eq!(tz, chrono_tz::America::New_York);
    }

    #[test]
    fn test_country_to_tz_jp() {
        let tz = country_to_tz("JP").unwrap();
        assert_eq!(tz, chrono_tz::Asia::Tokyo);
    }

    #[test]
    fn test_country_to_tz_unknown_returns_none() {
        assert!(country_to_tz("xx").is_none());
        assert!(country_to_tz("").is_none());
    }

    #[test]
    fn test_parse_request_utc_accepts_offset_bearing() {
        assert_eq!(
            parse_request_utc("2026-06-18T22:00:00+02:00")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
        assert_eq!(
            parse_request_utc("2026-06-18T20:00:00Z")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
    }

    #[test]
    fn test_parse_request_utc_rejects_zone_less_and_invalid() {
        // Zone-less and date-only inputs must be rejected at the API boundary.
        let err = parse_request_utc("2026-06-18 20:00:00").unwrap_err();
        assert!(err.contains("offset"), "got: {err}");
        assert!(parse_request_utc("2026-06-18T20:00:00").is_err());
        assert!(parse_request_utc("2026-06-18").is_err());
        assert!(parse_request_utc("not-a-date").is_err());
        assert!(parse_request_utc("").is_err());
    }
}
