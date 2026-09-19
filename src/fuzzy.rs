//! Forgiving title search: word by word, in any order, ignoring case, accents and punctuation,
//! and tolerating a slip of the finger. "alya hides" finds "Alya Sometimes Hides Her Feelings in
//! Russian"; "pokemon" finds "Pokémon"; "mentalsit" finds "The Mentalist".

/// Lowercase, fold common accents, turn everything that is not a letter or digit into a space.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let folded = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
            'ç' | 'ć' | 'č' => "c",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
            'ñ' | 'ń' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' => "o",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
            'ý' | 'ÿ' => "y",
            'š' | 'ś' => "s",
            'ž' | 'ź' | 'ż' => "z",
            'ł' => "l",
            'ø' => "o",
            'æ' => "ae",
            'ß' => "ss",
            'œ' => "oe",
            '\'' | '’' | '`' => "", // "don't" is one word
            c if c.is_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => " ",
        };
        out.push_str(folded);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Is the edit distance at most `max`? Swapped neighbours ("sit" for "ist") count as one slip
/// (optimal string alignment), and the scan stops early once a row can no longer get under `max`.
fn within(a: &str, b: &str, max: usize) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > max {
        return false;
    }
    let mut before: Vec<usize> = vec![];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let mut d = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(before[j - 2] + 1);
            }
            cur[j] = d;
        }
        if cur.iter().min().is_some_and(|m| *m > max) {
            return false;
        }
        before = std::mem::replace(&mut prev, cur);
    }
    prev[b.len()] <= max
}

/// How well one typed word matches any word of the title: 0 = not at all.
fn word_score(token: &str, words: &[&str]) -> i32 {
    let mut best = 0;
    // Short words must be typed right; longer ones may have one slip, long ones two.
    let slips = match token.chars().count() {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    };
    for w in words {
        let s = if *w == token {
            6
        } else if w.starts_with(token) {
            5
        } else if w.contains(token) {
            3
        } else if slips > 0 && (within(token, w, slips) || (w.chars().count() > token.chars().count() && within(token, &w.chars().take(token.chars().count()).collect::<String>(), slips))) {
            2
        } else {
            0
        };
        best = best.max(s);
    }
    best
}

/// A prepared query. Every word has to be found for a title to match at all.
pub struct Query {
    text: String,
    tokens: Vec<String>,
}

impl Query {
    pub fn new(raw: &str) -> Option<Self> {
        let text = normalize(raw);
        let tokens: Vec<String> = text.split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect();
        (!tokens.is_empty()).then_some(Query { text, tokens })
    }

    /// `None` = no match. Higher is better.
    pub fn score(&self, title: &str) -> Option<i32> {
        let hay = normalize(title);
        let words: Vec<&str> = hay.split(' ').collect();
        let mut total = 0;
        for t in &self.tokens {
            let s = word_score(t, &words);
            if s == 0 {
                return None;
            }
            total += s;
        }
        // "The Dark Knight" starts with "dark" as far as anyone searching is concerned.
        let hay = ["the ", "a ", "an "].iter().find_map(|art| hay.strip_prefix(art)).unwrap_or(&hay);
        if hay == self.text {
            total += 12;
        } else if hay.starts_with(&self.text) {
            total += 8;
        } else if hay.contains(&self.text) {
            total += 4; // the words as typed, next to each other
        }
        // Among equals, the shorter title is the likelier target.
        Some(total * 100 - (words.len() as i32).min(40))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(q: &str, title: &str) -> bool {
        Query::new(q).unwrap().score(title).is_some()
    }

    #[test]
    fn words_may_be_apart_and_in_any_order() {
        assert!(hit("alya hides", "Alya Sometimes Hides Her Feelings in Russian"));
        assert!(hit("russian alya", "Alya Sometimes Hides Her Feelings in Russian"));
        assert!(hit("rook", "The Rookery"));
        assert!(!hit("alya cooks", "Alya Sometimes Hides Her Feelings in Russian"), "every word has to be there");
    }

    #[test]
    fn accents_punctuation_and_case_do_not_matter() {
        assert!(hit("pokemon", "Pokémon: The First Movie"));
        assert!(hit("dont look", "Don't Look Up"));
        assert!(hit("spider man", "Spider-Man: No Way Home"));
        assert!(hit("WALL E", "WALL·E"));
    }

    #[test]
    fn a_slip_of_the_finger_is_forgiven_but_short_words_are_not_guessed() {
        assert!(hit("mentalsit", "The Mentalist"));
        assert!(hit("intersteller", "Interstellar"));
        assert!(hit("brekaing", "Breaking Bad"));
        assert!(!hit("cat", "Car Wars"), "three letters is too little to guess from");
        assert!(!hit("zzzzzz", "Interstellar"));
        assert!(within("mentalsit", "mentalist", 1) && !within("mentalsti", "mentalist", 1), "one swap is one slip; two are two");
    }

    #[test]
    fn better_matches_rank_higher() {
        let q = Query::new("dark").unwrap();
        let s = |t| q.score(t).unwrap();
        assert!(s("Dark") > s("Dark Matter"));
        assert!(s("Dark Matter") > s("The Dark Knight Rises Again"));
        assert!(s("The Dark Knight") > s("Darkness Falls Over Everything"), "a whole word beats a prefix of a longer one");
    }
}
