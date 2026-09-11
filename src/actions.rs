use gpui::actions;

actions!(
    git_helper,
    [
        OpenRepository,
        CloneRepository,
        CloseActiveRepository,
        NextRepository,
        PreviousRepository,
        RefreshRepository,
        ShowHistory,
        ShowChanges,
        ShowSummary,
        SummaryNextRow,
        SummaryPreviousRow,
        SummaryActivateRow,
        CreateCommit,
        GenerateCommitMessage,
    ]
);
