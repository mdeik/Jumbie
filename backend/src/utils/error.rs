// Error rendering helpers — SSoT for turning a captured error into a message
// that carries its full cause chain.

/// Render an error together with its full `source()` chain as a single string.
///
/// Some errors expose only a generic top-level message while the actionable
/// cause sits further down the chain. `reqwest` is the prime example: every
/// transport failure is reported as "error sending request for url (...)" and
/// the real reason (DNS resolution, connection refused, TLS handshake, timeout)
/// lives in its `source()`. Rendering the whole chain keeps that root cause
/// visible in logs and status messages instead of disappearing behind the
/// generic wrapper.
pub fn describe_error_chain(err: &dyn std::error::Error) -> String {
    let mut rendered = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Layer {
        msg: &'static str,
        source: Option<Box<Layer>>,
    }

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.msg)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_ref()
                .map(|s| s.as_ref() as &dyn std::error::Error)
        }
    }

    #[test]
    fn includes_every_link_in_the_chain() {
        let err = Layer {
            msg: "error sending request for url (https://nyaa.si/)",
            source: Some(Box::new(Layer {
                msg: "client error (Connect)",
                source: Some(Box::new(Layer {
                    msg: "dns error",
                    source: None,
                })),
            })),
        };
        assert_eq!(
            describe_error_chain(&err),
            "error sending request for url (https://nyaa.si/): client error (Connect): dns error"
        );
    }

    #[test]
    fn error_without_source_is_just_its_message() {
        let err = Layer {
            msg: "boom",
            source: None,
        };
        assert_eq!(describe_error_chain(&err), "boom");
    }
}
