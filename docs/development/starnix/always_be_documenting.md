# Always be documenting

Starnix is a complicated component with many subtle aspects to its development.
Starnix is also a part of the codebase that many people need to work on, even if
they are not on the Starnix team itself. Rather than teaching each of these
subtle aspects to each person individually, we should create more documentation
and refer people to that documentation more often. Hopefully, people will look
for information in docs, which will cause us to write more documentation,
creating a virtuous cycle.

## Answer questions with documents

Whenever the Starnix team receives a general question about how Starnix works or
how to develop Starnix, we should answer that question by writing some
documentation covering that topic and then refer the person asking the question
to that documentation. This approach creates the illusion of perfect
documentation coverage because the documentation is created "just in time" to
meet the needs of people asking questions.

This approach will result in many small fragments of documentation, which will
not be well-integrated into a whole. As these fragments accumulate, we should
refactor them into more comprehensive, better organized documentation. This
refactoring process is easier than writing documents from scratch because the
goal is not to add more information to the documentation, but to improve the
organization of the existing information.

## Practical steps

 1. **Identify the question**: When you type a paragraph explaining a complex
   topic in chat, stop.
 2. **Create a file**: Create a file in `//docs/development/starnix/`.
 3. **Paste and polish**: Paste your explanation. Add a title and a sentence of
    context.
 4. **Update TOC**: Add an entry for your new document in the `_toc.yaml` file.
    For more information, see
    [Updating site navigation and TOC files](/docs/contribute/docs/documentation-navigation-toc.md).
 5. **Publish**: Upload a CL. Send the link to the requester.
 6. **Refactor later**: Don't worry about where it fits perfectly. Getting the
    knowledge into the repo is the priority.

For more information on creating documentation, see [Contributing to documentation](/docs/contribute/docs/README.md)

## Imperfect documents

A rough, but focused document is better than no document. Don't worry if the
document is short or doesn't cover every possible edge case. If you are answering
a specific question, you can use the question as the title of the document.

## Organization

When creating a new document, you don't need to worry about finding the perfect
place for it in the documentation hierarchy. It is fine to add the file to the
top-level `starnix` directory or a generic `concepts` directory. As we accumulate
more documents, we can organize them into a more structured hierarchy.

## Coding agents

As a byproduct, this approach will also produce documentation that is useful to
coding agents working on our codebase. These coding agents benefit from this
documentation even more than humans because coding agents have more difficulty
asking live questions to humans.
