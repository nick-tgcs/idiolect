/// A single text-to-text translation request. The source text has already been
/// transcribed; languages are catalogue codes (see `idiolect-common`), with
/// `"auto"` allowed on the source side when the ASR engine detected the language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationRequest<'a> {
    pub text: &'a str,
    pub source_language: &'a str,
    pub target_language: &'a str,
}

pub trait TranslationPort {
    type Error;

    fn translate(&self, request: &TranslationRequest<'_>) -> Result<String, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::{TranslationPort, TranslationRequest};

    struct UppercaseTranslator;

    impl TranslationPort for UppercaseTranslator {
        type Error = std::convert::Infallible;

        fn translate(&self, request: &TranslationRequest<'_>) -> Result<String, Self::Error> {
            Ok(request.text.to_uppercase())
        }
    }

    #[test]
    fn port_carries_text_and_language_pair() {
        let request = TranslationRequest {
            text: "hej världen",
            source_language: "sv",
            target_language: "en",
        };
        assert_eq!(request.source_language, "sv");
        assert_eq!(request.target_language, "en");
        assert_eq!(
            UppercaseTranslator.translate(&request).expect("translate"),
            "HEJ VÄRLDEN"
        );
    }
}
