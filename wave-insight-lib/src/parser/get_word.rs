
pub fn get_word(c: char, word: String, last_type: LastType, line_idx: u32) -> (Option<(String,LastType)>,String,LastType,u32) {
    let mut word_out = word;
    match (c,&last_type) {
        ('\n',_) => {(Some((word_out,last_type)),"".to_string(),LastType::Space,line_idx+1)},

        (c,LastType::Normal) if is_word(c) => {word_out.push(c);(None,word_out,LastType::Normal,line_idx)},
        (c,LastType::Normal) if is_space(c) => {(Some((word_out,LastType::Normal)),"".to_string(),LastType::Space,line_idx)},
        (c,LastType::Normal) if is_operator(c) => {(Some((word_out,LastType::Normal)),c.to_string(),LastType::Operator,line_idx)},
        ('[',LastType::Normal) => {(Some((word_out,LastType::Normal)),"[".to_string(),LastType::Bracket,line_idx)},
        (']',LastType::Normal) => {(Some((word_out,LastType::Normal)),"]".to_string(),LastType::Bracket,line_idx)},//TODO:should raise error

        (c,LastType::Number) if is_word(c) => {word_out.push(c);(None,word_out,LastType::Number,line_idx)},
        (c,LastType::Number) if is_space(c) => {(Some((word_out,LastType::Number)),"".to_string(),LastType::Space,line_idx)},
        (c,LastType::Number) if is_operator(c) => {(Some((word_out,LastType::Number)),c.to_string(),LastType::Operator,line_idx)},
        ('[',LastType::Number) => {(Some((word_out,LastType::Number)),"[".to_string(),LastType::Bracket,line_idx)},
        (']',LastType::Number) => {(Some((word_out,LastType::Number)),"]".to_string(),LastType::Bracket,line_idx)},//TODO:should raise error

        (c,LastType::Operator) if is_operator(c) => {word_out.push(c);(None,word_out,LastType::Operator,line_idx)},
        (c,LastType::Operator) if is_space(c) => {(Some((word_out,LastType::Operator)),"".to_string(),LastType::Space,line_idx)},
        (c,LastType::Operator) if is_number_begin(c) => {(Some((word_out,LastType::Operator)),c.to_string(),LastType::Number,line_idx)},
        (c,LastType::Operator) if is_word(c) => {(Some((word_out,LastType::Operator)),c.to_string(),LastType::Normal,line_idx)},
        ('[',LastType::Operator) => {(Some((word_out,LastType::Operator)),"[".to_string(),LastType::Bracket,line_idx)},
        (']',LastType::Operator) => {(Some((word_out,LastType::Operator)),"]".to_string(),LastType::Bracket,line_idx)},//TODO:should raise error

        (c,LastType::Space) if is_number_begin(c) => {(None,c.to_string(),LastType::Number,line_idx)},
        (c,LastType::Space) if is_space(c) => {(None,word_out,LastType::Space,line_idx)},
        (c,LastType::Space) if is_word(c) => {(None,c.to_string(),LastType::Normal,line_idx)},
        (c,LastType::Space) if is_operator(c) => {(None,c.to_string(),LastType::Operator,line_idx)},
        ('[',LastType::Space) => {(None,"[".to_string(),LastType::Bracket,line_idx)},
        (']',LastType::Space) => {(None,"]".to_string(),LastType::Bracket,line_idx)},//TODO:should raise error

        (c,LastType::Bracket) if is_word(c) => {word_out.push(c);(None,word_out,LastType::Bracket,line_idx)},
        (c,LastType::Bracket) if is_space(c) => {word_out.push(c);(None,word_out,LastType::Bracket,line_idx)},
        (c,LastType::Bracket) if is_operator(c) => {word_out.push(c);(None,word_out,LastType::Bracket,line_idx)},
        ('[',LastType::Bracket) => {(None,"[".to_string(),LastType::Bracket,line_idx)},//TODO:should raise error?
        (']',LastType::Bracket) => {word_out.push(']');(Some((word_out,LastType::Bracket)),"".to_string(),LastType::Space,line_idx)},

        _ => {(None,word_out,LastType::Space,line_idx)},
    }
}

#[derive(Debug,Clone,Copy)]
pub enum LastType {
    Normal,
    Number,
    Operator,
    Space,
    Bracket,
}

fn is_word(c: char) -> bool {
    ('a'..='z').contains(&c)
        || ('A'..='Z').contains(&c)
        || ('0'..='9').contains(&c)
        || c == '_'

        || c == '\''//TODO:special for something like 3'b000, but maybe need some change
}

fn is_number_begin(c: char) -> bool {
    ('0'..='9').contains(&c)
}

fn is_space(c: char) -> bool {
    c == ' '
        
}

fn is_operator(c: char) -> bool {
    c == '<'
        || c == '>'
        || c == '='
        || c == '+'
        || c == '-'
        || c == '*'
        || c == '/'
        || c == '&'
        || c == '|'
        || c == '?'
        || c == ':'
        || c == '!'
        || c == '~'
        || c == '^'

        || c == ';'
        || c == ','
        || c == '.'
        || c == '@'
        || c == '#'
        || c == '$'
        || c == '%'

        || c == '('
        || c == ')'
        || c == '{'
        || c == '}'
}
