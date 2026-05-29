#![allow(unused)]

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Operator {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,

    LOr,
    LAnd,

    BOr,
    BAnd,
    BLShift,
    BRShift,

    Eq,
    NEq,
    Le,
    Lt,
    Ge,
    Gt,
    
    Neg,
    LNot,
    BNot,
}